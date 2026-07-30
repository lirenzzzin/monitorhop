use input_event::{ClipboardEvent, Event as InputEvent, KeyboardEvent, PointerEvent};
use num_enum::{IntoPrimitive, TryFromPrimitive, TryFromPrimitiveError};
use paste::paste;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt::{Debug, Display, Formatter},
    mem::size_of,
};
use thiserror::Error;

/// defines the maximum size a fixed-buffer encoded event can take up.
/// All non-clipboard events fit in this size; clipboard events use
/// [`encode_clipboard_transfer`] and [`decode_clipboard_frame`].
pub const MAX_EVENT_SIZE: usize = size_of::<u8>() + size_of::<u32>() + 2 * size_of::<f64>();

/// Maximum accepted UTF-8 clipboard text. Clipboard transfers are
/// split into small DTLS datagrams, so ordinary code snippets and
/// large multi-line documents do not depend on IP fragmentation.
pub const MAX_CLIPBOARD_SIZE: usize = 2 * 1024 * 1024;

/// Upper bound for an encoded clipboard protocol datagram.
pub const MAX_CLIPBOARD_DATAGRAM_SIZE: usize = 1200;

/// Application payload in each clipboard chunk. The remaining bytes
/// in [`MAX_CLIPBOARD_DATAGRAM_SIZE`] hold the type, transfer id,
/// chunk index and length.
pub const CLIPBOARD_CHUNK_SIZE: usize = 1120;

const MAX_FINGERPRINT_SIZE: usize = 256;

/// 8-byte protocol magic identifying a monitorhop peer, carried in
/// every [`ProtoEvent::Hello`]. The `Hello` is exchanged right after
/// the DTLS handshake authenticates; a peer that fails to present
/// this exact magic within the handshake window is not a monitorhop
/// instance and has its connection refused. monitorhop is deliberately
/// **not** wire-compatible with lan-mouse or any other fork — change
/// this magic to force a hard break against a future divergence.
pub const PROTOCOL_MAGIC: [u8; 8] = *b"MONHOP01";

/// error type for protocol violations
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// event type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidEventId(#[from] TryFromPrimitiveError<EventType>),
    /// position type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidPosition(#[from] TryFromPrimitiveError<Position>),
    /// clipboard payload exceeds [`MAX_CLIPBOARD_SIZE`]
    #[error("clipboard payload too large: {0} bytes")]
    ClipboardTooLarge(usize),
    /// clipboard text is not valid UTF-8
    #[error("invalid UTF-8 in clipboard payload")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    /// clipboard transfer metadata does not match the received chunks
    #[error("invalid clipboard transfer")]
    InvalidClipboardTransfer,
    /// clipboard transfer failed its SHA-256 integrity check
    #[error("clipboard integrity check failed")]
    ClipboardHashMismatch,
    /// not enough bytes left in the buffer
    #[error("buffer too small for clipboard payload")]
    BufferTooSmall,
}

/// Position of a client
#[derive(Clone, Copy, Debug, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum Position {
    Left,
    Right,
    Top,
    Bottom,
}

impl Display for Position {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let pos = match self {
            Position::Left => "left",
            Position::Right => "right",
            Position::Top => "top",
            Position::Bottom => "bottom",
        };
        write!(f, "{pos}")
    }
}

/// main monitorhop protocol event type
#[derive(Clone, Debug)]
pub enum ProtoEvent {
    /// notify a client that the cursor entered its region at the given position
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Enter(Position),
    /// notify a client that the cursor left its region
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Leave(u32),
    /// acknowledge of an [`ProtoEvent::Enter`] or [`ProtoEvent::Leave`] event
    Ack(u32),
    /// Input event
    Input(InputEvent),
    /// Ping event for tracking unresponsive clients.
    /// A client has to respond with [`ProtoEvent::Pong`].
    Ping,
    /// Response to [`ProtoEvent::Ping`], true if emulation is enabled / available
    Pong(bool),
    /// Display geometry of the receiving device. Sent by the
    /// emulation side immediately after the [`ProtoEvent::Ack`] of
    /// an [`ProtoEvent::Enter`] so the capturing peer can model the
    /// guest cursor's position along the entry axis. Width and
    /// height are in pixels of the union of all displays on the
    /// emulating device.
    Bounds { width: u32, height: u32 },
    /// Absolute cursor warp on the receiving device. Sent by the
    /// capturing peer after [`ProtoEvent::Enter`] so the guest's
    /// cursor lands at the position that visually corresponds to
    /// where the user's physical cursor was at the moment of
    /// crossing. `x` and `y` are pixel coordinates in the receiver's
    /// screen space, computed by the capturing peer using its own
    /// display bounds and the receiver-supplied [`ProtoEvent::Bounds`]
    /// from a prior Enter.
    MotionAbsolute { x: i32, y: i32 },
    /// Self-sufficient counterpart to [`ProtoEvent::MotionAbsolute`].
    /// Carries the host's cursor position normalized to the host's
    /// own display bounds (0..1 along each axis) plus the entry
    /// side from the receiver's frame. The receiver scales nx/ny
    /// against its own bounds and pins the on-axis dimension to
    /// the entry edge, eliminating the bootstrap problem where
    /// MotionAbsolute couldn't be sent on the first crossing
    /// because the host had no cached peer geometry.
    CursorPos { pos: Position, nx: f32, ny: f32 },
    /// Protocol handshake. Sent by the connect side immediately
    /// after the DTLS connection authenticates — retransmitted until
    /// the peer echoes one back — and mirrored by the listen side.
    /// `magic` must equal [`PROTOCOL_MAGIC`]; a peer that does not
    /// present a valid `Hello` within the handshake window has its
    /// connection refused. This is the deliberate hard cut-over that
    /// keeps monitorhop from silently half-interoperating with
    /// lan-mouse. `commit` is the 8-byte ASCII short commit hash
    /// from `shadow_rs`'s `SHORT_COMMIT`, surfaced in the GUI as the
    /// peer's build. Construct via [`ProtoEvent::hello`].
    Hello { magic: [u8; 8], commit: [u8; 8] },
    /// The receiver's per-pair motion-sensitivity multiplier.
    /// Sent by the emulating peer immediately after the
    /// [`ProtoEvent::Ack`] of an [`ProtoEvent::Enter`] so the
    /// capturing peer can scale its wall-press auto-release model
    /// to match. Without this, a sensitivity multiplier below 1.0
    /// would make the host's model accumulate "wall pressure"
    /// faster than the receiver's actual cursor moves, firing
    /// AutoRelease before the cursor has reached the edge. Old
    /// peers that don't recognize the event type silently skip it
    /// per the existing forward-compat handling.
    ReceiverSensitivity { mouse_sensitivity: f64 },
    /// Clipboard text content propagated from the originating peer.
    /// `from_fingerprint` is the TLS fingerprint of the peer that
    /// originally read the clipboard (not necessarily the sender —
    /// intermediate peers preserve the originator field when they
    /// fan-out to other peers). The receiver uses it to short-circuit
    /// the N-peer forwarding loop along with a recent-content cache.
    /// `content` is the clipboard text. It is split into acknowledged
    /// [`ClipboardFrame`] datagrams; the fixed-buffer codec panics on
    /// this logical variant.
    Clipboard {
        from_fingerprint: String,
        content: String,
    },
}

impl Display for ProtoEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoEvent::Enter(s) => write!(f, "Enter({s})"),
            ProtoEvent::Leave(s) => write!(f, "Leave({s})"),
            ProtoEvent::Ack(s) => write!(f, "Ack({s})"),
            ProtoEvent::Input(e) => write!(f, "{e}"),
            ProtoEvent::Ping => write!(f, "ping"),
            ProtoEvent::Pong(alive) => {
                write!(
                    f,
                    "pong: {}",
                    if *alive { "alive" } else { "not available" }
                )
            }
            ProtoEvent::Bounds { width, height } => write!(f, "Bounds({width}x{height})"),
            ProtoEvent::MotionAbsolute { x, y } => write!(f, "MotionAbsolute({x}, {y})"),
            ProtoEvent::CursorPos { pos, nx, ny } => {
                write!(f, "CursorPos({pos}, {nx:.4}, {ny:.4})")
            }
            ProtoEvent::ReceiverSensitivity { mouse_sensitivity } => {
                write!(f, "ReceiverSensitivity({mouse_sensitivity:.2})")
            }
            ProtoEvent::Hello { magic, commit } => {
                let s = std::str::from_utf8(commit).unwrap_or("????????");
                if *magic == PROTOCOL_MAGIC {
                    write!(f, "Hello({s})")
                } else {
                    write!(f, "Hello(foreign:{s})")
                }
            }
            ProtoEvent::Clipboard {
                from_fingerprint,
                content,
            } => {
                let head: String = content.chars().take(40).collect();
                let preview = if head.len() < content.len() {
                    format!("{head}…")
                } else {
                    head
                };
                write!(
                    f,
                    "Clipboard(from={}…, {}b: {preview})",
                    &from_fingerprint[..from_fingerprint.len().min(8)],
                    content.len(),
                )
            }
        }
    }
}

#[derive(TryFromPrimitive, IntoPrimitive, Debug)]
#[repr(u8)]
pub enum EventType {
    PointerMotion,
    PointerButton,
    PointerAxis,
    PointerAxisValue120,
    KeyboardKey,
    KeyboardModifiers,
    Ping,
    Pong,
    Enter,
    Leave,
    Ack,
    Bounds,
    MotionAbsolute,
    CursorPos,
    Hello,
    ReceiverSensitivity,
    /// Logical clipboard payload. This is split into
    /// [`ClipboardBegin`](EventType::ClipboardBegin) and
    /// [`ClipboardChunk`](EventType::ClipboardChunk) datagrams.
    Clipboard,
    ClipboardBegin,
    ClipboardChunk,
    ClipboardAck,
}

impl ProtoEvent {
    /// Construct a [`ProtoEvent::Hello`] stamped with this build's
    /// [`PROTOCOL_MAGIC`] and the given short commit hash.
    pub fn hello(commit: [u8; 8]) -> Self {
        ProtoEvent::Hello {
            magic: PROTOCOL_MAGIC,
            commit,
        }
    }

    fn event_type(&self) -> EventType {
        match self {
            ProtoEvent::Input(e) => match e {
                InputEvent::Pointer(p) => match p {
                    PointerEvent::Motion { .. } => EventType::PointerMotion,
                    PointerEvent::Button { .. } => EventType::PointerButton,
                    PointerEvent::Axis { .. } => EventType::PointerAxis,
                    PointerEvent::AxisDiscrete120 { .. } => EventType::PointerAxisValue120,
                },
                InputEvent::Keyboard(k) => match k {
                    KeyboardEvent::Key { .. } => EventType::KeyboardKey,
                    KeyboardEvent::Modifiers { .. } => EventType::KeyboardModifiers,
                },
                InputEvent::Clipboard(c) => match c {
                    ClipboardEvent::Text(_) => EventType::Clipboard,
                },
            },
            ProtoEvent::Ping => EventType::Ping,
            ProtoEvent::Pong(_) => EventType::Pong,
            ProtoEvent::Enter(_) => EventType::Enter,
            ProtoEvent::Leave(_) => EventType::Leave,
            ProtoEvent::Ack(_) => EventType::Ack,
            ProtoEvent::Bounds { .. } => EventType::Bounds,
            ProtoEvent::MotionAbsolute { .. } => EventType::MotionAbsolute,
            ProtoEvent::CursorPos { .. } => EventType::CursorPos,
            ProtoEvent::Hello { .. } => EventType::Hello,
            ProtoEvent::ReceiverSensitivity { .. } => EventType::ReceiverSensitivity,
            ProtoEvent::Clipboard { .. } => EventType::Clipboard,
        }
    }
}

impl TryFrom<[u8; MAX_EVENT_SIZE]> for ProtoEvent {
    type Error = ProtocolError;

    fn try_from(buf: [u8; MAX_EVENT_SIZE]) -> Result<Self, Self::Error> {
        let mut buf = &buf[..];
        let event_type = decode_u8(&mut buf)?;
        match EventType::try_from(event_type)? {
            EventType::PointerMotion => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Motion {
                    time: decode_u32(&mut buf)?,
                    dx: decode_f64(&mut buf)?,
                    dy: decode_f64(&mut buf)?,
                })))
            }
            EventType::PointerButton => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Button {
                    time: decode_u32(&mut buf)?,
                    button: decode_u32(&mut buf)?,
                    state: decode_u32(&mut buf)?,
                })))
            }
            EventType::PointerAxis => Ok(Self::Input(InputEvent::Pointer(PointerEvent::Axis {
                time: decode_u32(&mut buf)?,
                axis: decode_u8(&mut buf)?,
                value: decode_f64(&mut buf)?,
                momentum: decode_u8(&mut buf)? != 0,
            }))),
            EventType::PointerAxisValue120 => Ok(Self::Input(InputEvent::Pointer(
                PointerEvent::AxisDiscrete120 {
                    axis: decode_u8(&mut buf)?,
                    value: decode_i32(&mut buf)?,
                },
            ))),
            EventType::KeyboardKey => Ok(Self::Input(InputEvent::Keyboard(KeyboardEvent::Key {
                time: decode_u32(&mut buf)?,
                key: decode_u32(&mut buf)?,
                state: decode_u8(&mut buf)?,
            }))),
            EventType::KeyboardModifiers => Ok(Self::Input(InputEvent::Keyboard(
                KeyboardEvent::Modifiers {
                    depressed: decode_u32(&mut buf)?,
                    latched: decode_u32(&mut buf)?,
                    locked: decode_u32(&mut buf)?,
                    group: decode_u32(&mut buf)?,
                },
            ))),
            EventType::Ping => Ok(Self::Ping),
            EventType::Pong => Ok(Self::Pong(decode_u8(&mut buf)? != 0)),
            EventType::Enter => Ok(Self::Enter(decode_u8(&mut buf)?.try_into()?)),
            EventType::Leave => Ok(Self::Leave(decode_u32(&mut buf)?)),
            EventType::Ack => Ok(Self::Ack(decode_u32(&mut buf)?)),
            EventType::Bounds => Ok(Self::Bounds {
                width: decode_u32(&mut buf)?,
                height: decode_u32(&mut buf)?,
            }),
            EventType::MotionAbsolute => Ok(Self::MotionAbsolute {
                x: decode_i32(&mut buf)?,
                y: decode_i32(&mut buf)?,
            }),
            EventType::CursorPos => Ok(Self::CursorPos {
                pos: decode_u8(&mut buf)?.try_into()?,
                nx: decode_f32(&mut buf)?,
                ny: decode_f32(&mut buf)?,
            }),
            EventType::Hello => {
                let mut magic = [0u8; 8];
                for b in magic.iter_mut() {
                    *b = decode_u8(&mut buf)?;
                }
                let mut commit = [0u8; 8];
                for b in commit.iter_mut() {
                    *b = decode_u8(&mut buf)?;
                }
                Ok(Self::Hello { magic, commit })
            }
            EventType::ReceiverSensitivity => Ok(Self::ReceiverSensitivity {
                mouse_sensitivity: decode_f64(&mut buf)?,
            }),
            // Clipboard frames are variable-length and never arrive
            // through the fixed-size buffer path; the connect/listen
            // layer routes them through `decode_clipboard_frame`.
            EventType::Clipboard
            | EventType::ClipboardBegin
            | EventType::ClipboardChunk
            | EventType::ClipboardAck => Err(ProtocolError::BufferTooSmall),
        }
    }
}

impl From<ProtoEvent> for ([u8; MAX_EVENT_SIZE], usize) {
    fn from(event: ProtoEvent) -> Self {
        let mut buf = [0u8; MAX_EVENT_SIZE];
        let mut len = 0usize;
        {
            let mut buf = &mut buf[..];
            let buf = &mut buf;
            let len = &mut len;
            encode_u8(buf, len, event.event_type() as u8);
            match event {
                ProtoEvent::Input(event) => match event {
                    InputEvent::Pointer(p) => match p {
                        PointerEvent::Motion { time, dx, dy } => {
                            encode_u32(buf, len, time);
                            encode_f64(buf, len, dx);
                            encode_f64(buf, len, dy);
                        }
                        PointerEvent::Button {
                            time,
                            button,
                            state,
                        } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, button);
                            encode_u32(buf, len, state);
                        }
                        PointerEvent::Axis {
                            time,
                            axis,
                            value,
                            momentum,
                        } => {
                            encode_u32(buf, len, time);
                            encode_u8(buf, len, axis);
                            encode_f64(buf, len, value);
                            encode_u8(buf, len, momentum as u8);
                        }
                        PointerEvent::AxisDiscrete120 { axis, value } => {
                            encode_u8(buf, len, axis);
                            encode_i32(buf, len, value);
                        }
                    },
                    InputEvent::Keyboard(k) => match k {
                        KeyboardEvent::Key { time, key, state } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, key);
                            encode_u8(buf, len, state);
                        }
                        KeyboardEvent::Modifiers {
                            depressed,
                            latched,
                            locked,
                            group,
                        } => {
                            encode_u32(buf, len, depressed);
                            encode_u32(buf, len, latched);
                            encode_u32(buf, len, locked);
                            encode_u32(buf, len, group);
                        }
                    },
                    InputEvent::Clipboard(_) => {
                        panic!(
                            "ProtoEvent::Input(Clipboard) cannot use the fixed-buffer \
                             encoder; route via encode_clipboard_transfer"
                        );
                    }
                },
                ProtoEvent::Ping => {}
                ProtoEvent::Pong(alive) => encode_u8(buf, len, alive as u8),
                ProtoEvent::Enter(pos) => encode_u8(buf, len, pos as u8),
                ProtoEvent::Leave(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Ack(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Bounds { width, height } => {
                    encode_u32(buf, len, width);
                    encode_u32(buf, len, height);
                }
                ProtoEvent::MotionAbsolute { x, y } => {
                    encode_i32(buf, len, x);
                    encode_i32(buf, len, y);
                }
                ProtoEvent::CursorPos { pos, nx, ny } => {
                    encode_u8(buf, len, pos as u8);
                    encode_f32(buf, len, nx);
                    encode_f32(buf, len, ny);
                }
                ProtoEvent::Hello { magic, commit } => {
                    for b in magic.iter() {
                        encode_u8(buf, len, *b);
                    }
                    for b in commit.iter() {
                        encode_u8(buf, len, *b);
                    }
                }
                ProtoEvent::ReceiverSensitivity { mouse_sensitivity } => {
                    encode_f64(buf, len, mouse_sensitivity);
                }
                ProtoEvent::Clipboard { .. } => {
                    panic!(
                        "ProtoEvent::Clipboard cannot use the fixed-buffer encoder; \
                         route via encode_clipboard_transfer"
                    );
                }
            }
        }
        (buf, len)
    }
}

macro_rules! decode_impl {
    ($t:ty) => {
        paste! {
            fn [<decode_ $t>](data: &mut &[u8]) -> Result<$t, ProtocolError> {
                let (int_bytes, rest) = data.split_at(size_of::<$t>());
                *data = rest;
                Ok($t::from_be_bytes(int_bytes.try_into().unwrap()))
            }
        }
    };
}

decode_impl!(u8);
decode_impl!(u32);
decode_impl!(i32);
decode_impl!(f32);
decode_impl!(f64);

macro_rules! encode_impl {
    ($t:ty) => {
        paste! {
            fn [<encode_ $t>](buf: &mut &mut [u8], amt: &mut usize, n: $t) {
                let src = n.to_be_bytes();
                let data = std::mem::take(buf);
                let (int_bytes, rest) = data.split_at_mut(size_of::<$t>());
                int_bytes.copy_from_slice(&src);
                *amt += size_of::<$t>();
                *buf = rest
            }
        }
    };
}

encode_impl!(u8);
encode_impl!(u32);
encode_impl!(i32);
encode_impl!(f32);
encode_impl!(f64);

/// A single clipboard transfer datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardFrame {
    Begin {
        transfer_id: u64,
        from_fingerprint: String,
        content_len: u32,
        chunk_count: u32,
        content_hash: [u8; 32],
    },
    Chunk {
        transfer_id: u64,
        index: u32,
        data: Vec<u8>,
    },
    Ack {
        transfer_id: u64,
        content_hash: [u8; 32],
        accepted: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardTransfer {
    pub transfer_id: u64,
    pub from_fingerprint: String,
    pub content: String,
    pub content_hash: [u8; 32],
}

/// Split a logical clipboard event into MTU-safe datagrams.
///
/// The caller retransmits the returned datagrams until it receives a
/// matching [`ClipboardFrame::Ack`]. SHA-256 protects reassembly from
/// missing, duplicated, reordered or corrupted chunks.
pub fn encode_clipboard_transfer(
    event: &ProtoEvent,
    transfer_id: u64,
) -> Result<(Vec<Vec<u8>>, [u8; 32]), ProtocolError> {
    let (from_fingerprint, content) = match event {
        ProtoEvent::Clipboard {
            from_fingerprint,
            content,
        } => (from_fingerprint.as_str(), content.as_str()),
        ProtoEvent::Input(InputEvent::Clipboard(ClipboardEvent::Text(content))) => {
            // Convenience: capture-side callers carry only the text;
            // the originator fingerprint is empty until the service
            // layer stamps it in. Phase 2 wires the stamp.
            ("", content.as_str())
        }
        _ => panic!("encode_clipboard_transfer called on non-clipboard event"),
    };
    let fp_bytes = from_fingerprint.as_bytes();
    let text_bytes = content.as_bytes();
    if fp_bytes.len() > MAX_FINGERPRINT_SIZE || text_bytes.len() > MAX_CLIPBOARD_SIZE {
        return Err(ProtocolError::ClipboardTooLarge(text_bytes.len()));
    }
    let content_hash: [u8; 32] = Sha256::digest(text_bytes).into();
    let chunk_count = text_bytes.len().div_ceil(CLIPBOARD_CHUNK_SIZE).max(1) as u32;
    let begin = ClipboardFrame::Begin {
        transfer_id,
        from_fingerprint: from_fingerprint.to_owned(),
        content_len: text_bytes.len() as u32,
        chunk_count,
        content_hash,
    };
    let mut datagrams = Vec::with_capacity(chunk_count as usize + 1);
    datagrams.push(encode_clipboard_frame(&begin)?);
    if text_bytes.is_empty() {
        datagrams.push(encode_clipboard_frame(&ClipboardFrame::Chunk {
            transfer_id,
            index: 0,
            data: Vec::new(),
        })?);
    } else {
        for (index, data) in text_bytes.chunks(CLIPBOARD_CHUNK_SIZE).enumerate() {
            datagrams.push(encode_clipboard_frame(&ClipboardFrame::Chunk {
                transfer_id,
                index: index as u32,
                data: data.to_vec(),
            })?);
        }
    }
    Ok((datagrams, content_hash))
}

pub fn encode_clipboard_frame(frame: &ClipboardFrame) -> Result<Vec<u8>, ProtocolError> {
    let mut buf = Vec::with_capacity(MAX_CLIPBOARD_DATAGRAM_SIZE);
    match frame {
        ClipboardFrame::Begin {
            transfer_id,
            from_fingerprint,
            content_len,
            chunk_count,
            content_hash,
        } => {
            let fingerprint = from_fingerprint.as_bytes();
            if fingerprint.len() > MAX_FINGERPRINT_SIZE
                || *content_len as usize > MAX_CLIPBOARD_SIZE
                || *chunk_count == 0
            {
                return Err(ProtocolError::InvalidClipboardTransfer);
            }
            buf.push(EventType::ClipboardBegin as u8);
            buf.extend_from_slice(&transfer_id.to_be_bytes());
            buf.extend_from_slice(&(fingerprint.len() as u16).to_be_bytes());
            buf.extend_from_slice(fingerprint);
            buf.extend_from_slice(&content_len.to_be_bytes());
            buf.extend_from_slice(&chunk_count.to_be_bytes());
            buf.extend_from_slice(content_hash);
        }
        ClipboardFrame::Chunk {
            transfer_id,
            index,
            data,
        } => {
            if data.len() > CLIPBOARD_CHUNK_SIZE {
                return Err(ProtocolError::ClipboardTooLarge(data.len()));
            }
            buf.push(EventType::ClipboardChunk as u8);
            buf.extend_from_slice(&transfer_id.to_be_bytes());
            buf.extend_from_slice(&index.to_be_bytes());
            buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
            buf.extend_from_slice(data);
        }
        ClipboardFrame::Ack {
            transfer_id,
            content_hash,
            accepted,
        } => {
            buf.push(EventType::ClipboardAck as u8);
            buf.extend_from_slice(&transfer_id.to_be_bytes());
            buf.extend_from_slice(content_hash);
            buf.push(u8::from(*accepted));
        }
    }
    if buf.len() > MAX_CLIPBOARD_DATAGRAM_SIZE {
        return Err(ProtocolError::ClipboardTooLarge(buf.len()));
    }
    Ok(buf)
}

pub fn decode_clipboard_frame(buf: &[u8]) -> Result<ClipboardFrame, ProtocolError> {
    if buf.is_empty() || buf.len() > MAX_CLIPBOARD_DATAGRAM_SIZE {
        return Err(ProtocolError::BufferTooSmall);
    }
    let mut cursor = 1usize;
    let event_type = EventType::try_from(buf[0])?;
    match event_type {
        EventType::ClipboardBegin => {
            let transfer_id = read_u64(buf, &mut cursor)?;
            let fp_len = read_u16(buf, &mut cursor)? as usize;
            if fp_len > MAX_FINGERPRINT_SIZE || buf.len() < cursor + fp_len {
                return Err(ProtocolError::InvalidClipboardTransfer);
            }
            let from_fingerprint = String::from_utf8(buf[cursor..cursor + fp_len].to_vec())?;
            cursor += fp_len;
            let content_len = read_u32(buf, &mut cursor)?;
            let chunk_count = read_u32(buf, &mut cursor)?;
            let content_hash = read_array::<32>(buf, &mut cursor)?;
            if content_len as usize > MAX_CLIPBOARD_SIZE
                || chunk_count == 0
                || chunk_count as usize
                    != (content_len as usize).div_ceil(CLIPBOARD_CHUNK_SIZE).max(1)
            {
                return Err(ProtocolError::InvalidClipboardTransfer);
            }
            Ok(ClipboardFrame::Begin {
                transfer_id,
                from_fingerprint,
                content_len,
                chunk_count,
                content_hash,
            })
        }
        EventType::ClipboardChunk => {
            let transfer_id = read_u64(buf, &mut cursor)?;
            let index = read_u32(buf, &mut cursor)?;
            let len = read_u16(buf, &mut cursor)? as usize;
            if len > CLIPBOARD_CHUNK_SIZE || buf.len() != cursor + len {
                return Err(ProtocolError::InvalidClipboardTransfer);
            }
            Ok(ClipboardFrame::Chunk {
                transfer_id,
                index,
                data: buf[cursor..].to_vec(),
            })
        }
        EventType::ClipboardAck => {
            let transfer_id = read_u64(buf, &mut cursor)?;
            let content_hash = read_array::<32>(buf, &mut cursor)?;
            let accepted = read_u8_slice(buf, &mut cursor)? != 0;
            if cursor != buf.len() {
                return Err(ProtocolError::InvalidClipboardTransfer);
            }
            Ok(ClipboardFrame::Ack {
                transfer_id,
                content_hash,
                accepted,
            })
        }
        _ => Err(ProtocolError::InvalidClipboardTransfer),
    }
}

fn read_array<const N: usize>(buf: &[u8], cursor: &mut usize) -> Result<[u8; N], ProtocolError> {
    if buf.len() < *cursor + N {
        return Err(ProtocolError::BufferTooSmall);
    }
    let out = buf[*cursor..*cursor + N]
        .try_into()
        .map_err(|_| ProtocolError::BufferTooSmall)?;
    *cursor += N;
    Ok(out)
}

fn read_u8_slice(buf: &[u8], cursor: &mut usize) -> Result<u8, ProtocolError> {
    Ok(read_array::<1>(buf, cursor)?[0])
}

fn read_u16(buf: &[u8], cursor: &mut usize) -> Result<u16, ProtocolError> {
    Ok(u16::from_be_bytes(read_array(buf, cursor)?))
}

fn read_u32(buf: &[u8], cursor: &mut usize) -> Result<u32, ProtocolError> {
    Ok(u32::from_be_bytes(read_array(buf, cursor)?))
}

fn read_u64(buf: &[u8], cursor: &mut usize) -> Result<u64, ProtocolError> {
    Ok(u64::from_be_bytes(read_array(buf, cursor)?))
}

#[derive(Debug)]
struct PendingClipboard {
    from_fingerprint: String,
    content_len: usize,
    content_hash: [u8; 32],
    chunks: Vec<Option<Vec<u8>>>,
}

/// Reassembles out-of-order and duplicate clipboard chunks.
#[derive(Default, Debug)]
pub struct ClipboardAssembler {
    pending: HashMap<u64, PendingClipboard>,
}

impl ClipboardAssembler {
    pub fn push(
        &mut self,
        frame: ClipboardFrame,
    ) -> Result<Option<ClipboardTransfer>, ProtocolError> {
        match frame {
            ClipboardFrame::Begin {
                transfer_id,
                from_fingerprint,
                content_len,
                chunk_count,
                content_hash,
            } => {
                let pending = PendingClipboard {
                    from_fingerprint,
                    content_len: content_len as usize,
                    content_hash,
                    chunks: vec![None; chunk_count as usize],
                };
                match self.pending.get(&transfer_id) {
                    Some(existing)
                        if existing.content_len == pending.content_len
                            && existing.content_hash == pending.content_hash
                            && existing.from_fingerprint == pending.from_fingerprint =>
                    {
                        // Retransmitted begin frame: retain chunks already received.
                    }
                    _ => {
                        self.pending.insert(transfer_id, pending);
                    }
                }
                Ok(None)
            }
            ClipboardFrame::Chunk {
                transfer_id,
                index,
                data,
            } => {
                let Some(pending) = self.pending.get_mut(&transfer_id) else {
                    // A reordered chunk can precede Begin. The sender retransmits
                    // the complete transfer after a missed acknowledgement.
                    return Ok(None);
                };
                let Some(slot) = pending.chunks.get_mut(index as usize) else {
                    return Err(ProtocolError::InvalidClipboardTransfer);
                };
                if slot.is_none() {
                    *slot = Some(data);
                }
                if pending.chunks.iter().any(Option::is_none) {
                    return Ok(None);
                }
                let mut bytes = Vec::with_capacity(pending.content_len);
                for chunk in &pending.chunks {
                    bytes.extend_from_slice(chunk.as_deref().expect("all chunks present"));
                }
                if bytes.len() != pending.content_len {
                    self.pending.remove(&transfer_id);
                    return Err(ProtocolError::InvalidClipboardTransfer);
                }
                let actual_hash: [u8; 32] = Sha256::digest(&bytes).into();
                if actual_hash != pending.content_hash {
                    self.pending.remove(&transfer_id);
                    return Err(ProtocolError::ClipboardHashMismatch);
                }
                let from_fingerprint = pending.from_fingerprint.clone();
                let content_hash = pending.content_hash;
                self.pending.remove(&transfer_id);
                Ok(Some(ClipboardTransfer {
                    transfer_id,
                    from_fingerprint,
                    content: String::from_utf8(bytes)?,
                    content_hash,
                }))
            }
            ClipboardFrame::Ack { .. } => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_round_trip() {
        let event = ProtoEvent::Clipboard {
            from_fingerprint: "abcd1234".into(),
            content: "hello, world".into(),
        };
        let (datagrams, hash) = encode_clipboard_transfer(&event, 42).expect("encode");
        assert!(
            datagrams
                .iter()
                .all(|d| d.len() <= MAX_CLIPBOARD_DATAGRAM_SIZE)
        );
        let mut assembler = ClipboardAssembler::default();
        let mut transfer = None;
        for datagram in datagrams {
            let frame = decode_clipboard_frame(&datagram).expect("decode");
            transfer = assembler.push(frame).expect("assemble").or(transfer);
        }
        let transfer = transfer.expect("completed transfer");
        assert_eq!(transfer.transfer_id, 42);
        assert_eq!(transfer.from_fingerprint, "abcd1234");
        assert_eq!(transfer.content, "hello, world");
        assert_eq!(transfer.content_hash, hash);
    }

    #[test]
    fn clipboard_too_large_rejected() {
        let event = ProtoEvent::Clipboard {
            from_fingerprint: "fp".into(),
            content: "x".repeat(MAX_CLIPBOARD_SIZE + 1),
        };
        assert!(matches!(
            encode_clipboard_transfer(&event, 1),
            Err(ProtocolError::ClipboardTooLarge(_))
        ));
    }

    #[test]
    fn clipboard_large_unicode_round_trip_out_of_order() {
        let content = "ação 🚀\n".repeat(40_000);
        let event = ProtoEvent::Clipboard {
            from_fingerprint: "fp".into(),
            content: content.clone(),
        };
        let (datagrams, _) = encode_clipboard_transfer(&event, 7).expect("encode");
        let begin = decode_clipboard_frame(&datagrams[0]).expect("begin");
        let mut chunks: Vec<_> = datagrams[1..]
            .iter()
            .map(|d| decode_clipboard_frame(d).expect("chunk"))
            .collect();
        chunks.reverse();
        let mut assembler = ClipboardAssembler::default();
        assembler.push(begin).expect("begin accepted");
        let mut completed = None;
        for chunk in chunks {
            completed = assembler.push(chunk).expect("chunk accepted").or(completed);
        }
        assert_eq!(completed.expect("complete").content, content);
    }

    #[test]
    fn clipboard_ack_round_trip() {
        let frame = ClipboardFrame::Ack {
            transfer_id: 99,
            content_hash: [7; 32],
            accepted: true,
        };
        let bytes = encode_clipboard_frame(&frame).expect("encode");
        assert_eq!(decode_clipboard_frame(&bytes).expect("decode"), frame);
    }

    #[test]
    fn hello_round_trip_carries_magic() {
        let (buf, len): ([u8; MAX_EVENT_SIZE], usize) = ProtoEvent::hello(*b"abcd1234").into();
        assert!(len <= MAX_EVENT_SIZE);
        match buf.try_into().expect("decode") {
            ProtoEvent::Hello { magic, commit } => {
                assert_eq!(magic, PROTOCOL_MAGIC);
                assert_eq!(&commit, b"abcd1234");
            }
            other => panic!("expected Hello, got {other}"),
        }
    }

    #[test]
    fn foreign_hello_decodes_but_magic_mismatches() {
        // A Hello from a non-monitorhop peer still decodes — the
        // connection layer is what rejects it, on the magic.
        let foreign = ProtoEvent::Hello {
            magic: *b"LAN-MOUS",
            commit: *b"00000000",
        };
        let (buf, len): ([u8; MAX_EVENT_SIZE], usize) = foreign.into();
        assert!(len <= MAX_EVENT_SIZE);
        let decoded: ProtoEvent = buf.try_into().expect("decode");
        assert!(!matches!(
            decoded,
            ProtoEvent::Hello { magic, .. } if magic == PROTOCOL_MAGIC
        ));
    }
}
