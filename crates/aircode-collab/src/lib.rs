//! Совместная работа в AIRcode поверх серверов uchi-on-line (Django Channels).
//!
//! Два независимых канала одной комнаты:
//!
//! * [`CollabSession`] — совместное редактирование кода. Клиент бинарного
//!   релея `/ws/code/<room_id>/` (см. `codepad/consumers.py` в uchi-on-line):
//!   CRDT-документ [`yrs`], бинарно совместимый с браузерным Yjs, — десктоп
//!   и веб-кодпад редактируют один документ одновременно.
//!
//! * [`voice`] (feature `voice`) — голосовые звонки. Клиент сигналинга
//!   `/ws/video/<room_id>/` (см. `video/consumers.py` и `static/js/video.js`):
//!   WebRTC mesh «каждый с каждым», аудио G.711 PCMU.
//!
//! Аутентификация в обоих случаях — Django-сессионная cookie
//! (`sessionid`), полученная логином на веб-сервисе.

pub mod actor;
pub mod protocol;
pub mod session;
pub mod sync;

#[cfg(feature = "voice")]
pub mod audio;
#[cfg(feature = "voice")]
pub mod voice;

pub use actor::{CollabCommand, CollabConfig, CollabEvent, CollabHandle};
pub use session::{CollabSession, PeerInfo};
pub use sync::{bytes_to_utf16_units, diff_texts, CollabBinding, TextDiff};
