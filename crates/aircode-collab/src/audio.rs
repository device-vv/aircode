//! Захват и воспроизведение звука для голосовых звонков (cpal).
//!
//! Линия голоса — G.711 PCMU 8 кГц mono; устройства обычно дают 44.1/48 кГц,
//! поэтому здесь же простой линейный ресэмплинг. Качества «телефонного
//! канала» (а PCMU — это буквально телефония) ему достаточно.
//!
//! Живут объекты на потоке актора: cpal-потоки на Windows `Send`, но
//! создаём/роняем их строго там же, где создали.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

/// Частота линии голоса (PCMU).
pub const VOICE_RATE: u32 = 8000;
/// Размер кадра линии: 20 мс.
pub const FRAME_SAMPLES: usize = 160;

fn to_f32(sample_format: cpal::SampleFormat) -> anyhow::Result<SampleReader> {
    Ok(match sample_format {
        cpal::SampleFormat::F32 => SampleReader::F32,
        cpal::SampleFormat::I16 => SampleReader::I16,
        cpal::SampleFormat::U16 => SampleReader::U16,
        other => anyhow::bail!("неподдерживаемый формат сэмплов: {other}"),
    })
}

enum SampleReader {
    F32,
    I16,
    U16,
}

/// Микрофон: устройство ввода -> PCM i16 8 кГц mono в канал [`VoiceCall`].
pub struct MicStream {
    _stream: cpal::Stream,
    muted: Arc<AtomicBool>,
}

impl MicStream {
    /// Начать захват с устройства по умолчанию. Кадры по 160 сэмплов
    /// уходят в `pcm_tx`. При ошибке устройства — `Err`.
    pub fn start(pcm_tx: mpsc::UnboundedSender<Vec<i16>>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("нет устройства ввода (микрофон)"))?;
        let config = device.default_input_config()?;
        let channels = config.channels() as usize;
        let src_rate = config.sample_rate().0;
        let reader = to_f32(config.sample_format())?;
        let muted = Arc::new(AtomicBool::new(false));
        let muted_cb = Arc::clone(&muted);

        // Состояние ресэмплера: хвост моно-буфера + дробная фаза
        // (сохраняется между колбэками, иначе накопится дрейф).
        let mut resampler = Downsampler::new(src_rate);
        let mut out_pending: Vec<i16> = Vec::new();
        let err_fn = |e| eprintln!("[voice] mic error: {e}");

        let stream = match reader {
            SampleReader::F32 => device.build_input_stream(
                &config.config(),
                move |data: &[f32], _| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|c| c.iter().sum::<f32>() / channels as f32)
                        .collect();
                    push_mic_frame(mono, &mut resampler, &muted_cb, &mut out_pending, &pcm_tx);
                },
                err_fn,
                None,
            )?,
            SampleReader::I16 => device.build_input_stream(
                &config.config(),
                move |data: &[i16], _| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|c| {
                            c.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    push_mic_frame(mono, &mut resampler, &muted_cb, &mut out_pending, &pcm_tx);
                },
                err_fn,
                None,
            )?,
            SampleReader::U16 => device.build_input_stream(
                &config.config(),
                move |data: &[u16], _| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|c| {
                            c.iter()
                                .map(|&s| (s as f32 - 32768.0) / 32768.0)
                                .sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    push_mic_frame(mono, &mut resampler, &muted_cb, &mut out_pending, &pcm_tx);
                },
                err_fn,
                None,
            )?,
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            muted,
        })
    }

    /// Выключить/включить микрофон (в линию уходит тишина).
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// Текущее состояние мьютa.
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }
}

/// Дециматор device-rate -> 8 кГц с линейной интерполяцией и
/// persistent-фазой между вызовами.
struct Downsampler {
    ratio: f64,
    phase: f64,
    tail: Vec<f32>,
}

impl Downsampler {
    fn new(src_rate: u32) -> Self {
        Self {
            ratio: src_rate as f64 / VOICE_RATE as f64,
            phase: 0.0,
            tail: Vec::new(),
        }
    }

    /// Добавить моно-сэмплы устройства, вернуть готовые сэмплы 8 кГц.
    fn push(&mut self, mono: &[f32]) -> Vec<i16> {
        self.tail.extend_from_slice(mono);
        let mut out = Vec::new();
        while (self.phase.floor() as usize) + 1 < self.tail.len() {
            let idx = self.phase.floor() as usize;
            let frac = (self.phase - idx as f64) as f32;
            let a = self.tail[idx];
            let b = self.tail[idx + 1];
            let sample = a + (b - a) * frac;
            out.push((sample * i16::MAX as f32).clamp(-32768.0, 32767.0) as i16);
            self.phase += self.ratio;
        }
        let consumed = self.phase.floor() as usize;
        self.tail.drain(..consumed.min(self.tail.len()));
        self.phase -= consumed as f64;
        out
    }
}

/// Ресэмплинг device-rate -> 8 кГц и нарезка кадрами по 160 сэмплов.
fn push_mic_frame(
    mono: Vec<f32>,
    resampler: &mut Downsampler,
    muted: &AtomicBool,
    out_pending: &mut Vec<i16>,
    pcm_tx: &mpsc::UnboundedSender<Vec<i16>>,
) {
    let mut samples = resampler.push(&mono);
    if muted.load(Ordering::Relaxed) {
        for s in samples.iter_mut() {
            *s = 0;
        }
    }
    out_pending.extend_from_slice(&samples);
    while out_pending.len() >= FRAME_SAMPLES {
        let frame: Vec<i16> = out_pending.drain(..FRAME_SAMPLES).collect();
        let _ = pcm_tx.send(frame);
    }
}

/// Динамик: канал PCM 8 кГц -> устройство вывода по умолчанию.
pub struct SpeakerStream {
    _stream: cpal::Stream,
    _pump: std::thread::JoinHandle<()>,
}

impl SpeakerStream {
    /// Начать воспроизведение кадров из `pcm_rx` на устройстве по умолчанию.
    pub fn start(mut pcm_rx: mpsc::UnboundedReceiver<Vec<i16>>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("нет устройства вывода (динамик)"))?;
        let config = device.default_output_config()?;
        let channels = config.channels() as usize;
        let dst_rate = config.sample_rate().0;
        let reader = to_f32(config.sample_format())?;

        // Буфер сэмплов 8 кГц; наполняется из канала, читается колбэком.
        let queue: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));
        let queue_writer = Arc::clone(&queue);
        let pump = std::thread::Builder::new()
            .name("aircode-voice-out".into())
            .spawn(move || {
                while let Some(frame) = pcm_rx.blocking_recv() {
                    let mut q = queue_writer.lock().unwrap();
                    q.extend(frame);
                    // Защита от разрастания при джиттере: держим ≤ 1 с.
                    while q.len() > VOICE_RATE as usize {
                        q.pop_front();
                    }
                }
            })?;

        // Состояние апсэмплинга 8 кГц -> device rate.
        let pos = Arc::new(Mutex::new(0.0f64));
        let pos_cb = Arc::clone(&pos);
        let queue_cb = Arc::clone(&queue);
        let err_fn = |e| eprintln!("[voice] speaker error: {e}");

        let fill = move |out: &mut [f32]| {
            let mut q = queue_cb.lock().unwrap();
            let mut p = pos_cb.lock().unwrap();
            let step = VOICE_RATE as f64 / dst_rate as f64;
            for frame in out.chunks_mut(channels) {
                let idx = *p as usize;
                let frac = (*p - idx as f64) as f32;
                let a = q.get(idx).copied().unwrap_or(0) as f32 / i16::MAX as f32;
                let b = q.get(idx + 1).copied().unwrap_or(a as i16) as f32 / i16::MAX as f32;
                let sample = a + (b - a) * frac;
                for s in frame.iter_mut() {
                    *s = sample;
                }
                *p += step;
            }
            let consumed = *p as usize;
            *p -= consumed as f64;
            for _ in 0..consumed.min(q.len()) {
                q.pop_front();
            }
        };

        let stream = match reader {
            SampleReader::F32 => {
                let fill = fill;
                device.build_output_stream(
                    &config.config(),
                    move |data: &mut [f32], _| fill(data),
                    err_fn,
                    None,
                )?
            }
            SampleReader::I16 => {
                let fill = fill;
                device.build_output_stream(
                    &config.config(),
                    move |data: &mut [i16], _| {
                        // Пишем через промежуточный f32-буфер.
                        let mut tmp = vec![0.0f32; data.len()];
                        fill(&mut tmp);
                        for (d, t) in data.iter_mut().zip(tmp) {
                            *d = (t * i16::MAX as f32).clamp(-32768.0, 32767.0) as i16;
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleReader::U16 => {
                let fill = fill;
                device.build_output_stream(
                    &config.config(),
                    move |data: &mut [u16], _| {
                        let mut tmp = vec![0.0f32; data.len()];
                        fill(&mut tmp);
                        for (d, t) in data.iter_mut().zip(tmp) {
                            *d = ((t * 32768.0) + 32768.0).clamp(0.0, 65535.0) as u16;
                        }
                    },
                    err_fn,
                    None,
                )?
            }
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            _pump: pump,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsampler_produces_frame() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let muted = AtomicBool::new(false);
        let mut resampler = Downsampler::new(48000);
        let mut pending = Vec::new();
        // 48 кГц, 480 сэмплов (10 мс) -> ровно 80 сэмплов 8 кГц.
        let input = vec![0.5f32; 480];
        push_mic_frame(input, &mut resampler, &muted, &mut pending, &tx);
        let input = vec![0.5f32; 480];
        push_mic_frame(input, &mut resampler, &muted, &mut pending, &tx);
        // За 20 мс входа должен собраться один кадр 160 сэмплов.
        let frame = rx.try_recv().expect("frame");
        assert_eq!(frame.len(), FRAME_SAMPLES);
        assert!(frame.iter().all(|&s| (s - 16383).abs() <= 2));
    }

    #[test]
    fn muted_mic_sends_silence() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let muted = AtomicBool::new(true);
        let mut resampler = Downsampler::new(48000);
        let mut pending = Vec::new();
        let input = vec![1.0f32; 960];
        push_mic_frame(input, &mut resampler, &muted, &mut pending, &tx);
        let frame = rx.try_recv().expect("frame");
        assert!(frame.iter().all(|&s| s == 0));
    }
}
