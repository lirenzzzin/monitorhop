use clipboard_rs::{Clipboard, ClipboardContext, common::RustImage};
use input_event::ClipboardEvent;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::task::spawn_blocking;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("Failed to access clipboard: {0}")]
    Access(String),
    #[error("Failed to set clipboard: {0}")]
    Set(String),
}

/// Clipboard emulation that publishes text, PNG images, or native file URI
/// lists. The platform library owns the X11/Wayland/Windows/macOS details.
#[derive(Clone)]
pub struct ClipboardEmulation {
    clipboard: Arc<Mutex<Option<ClipboardContext>>>,
}

impl ClipboardEmulation {
    pub fn new() -> Result<Self, ClipboardError> {
        let clipboard = match ClipboardContext::new() {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("Failed to create clipboard instance: {e}");
                None
            }
        };

        Ok(Self {
            clipboard: Arc::new(Mutex::new(clipboard)),
        })
    }

    /// Set clipboard content from a validated clipboard event.
    pub async fn set(&self, event: ClipboardEvent) -> Result<(), ClipboardError> {
        let clipboard_arc = self.clipboard.clone();

        spawn_blocking(move || {
            let mut clipboard_guard = clipboard_arc.lock().unwrap();
            let clipboard = match clipboard_guard.as_mut() {
                Some(c) => c,
                None => {
                    let c = ClipboardContext::new()
                        .map_err(|e| ClipboardError::Access(format!("{e}")))?;
                    *clipboard_guard = Some(c);
                    clipboard_guard.as_mut().expect("clipboard inserted")
                }
            };

            match event {
                ClipboardEvent::Text(text) => clipboard
                    .set_text(text)
                    .map_err(|e| ClipboardError::Set(format!("{e}")))?,
                ClipboardEvent::ImagePng(png) => {
                    let image = clipboard_rs::common::RustImageData::from_bytes(&png)
                        .map_err(|e| ClipboardError::Set(format!("{e}")))?;
                    clipboard
                        .set_image(image)
                        .map_err(|e| ClipboardError::Set(format!("{e}")))?;
                }
                ClipboardEvent::Files(files) => clipboard
                    .set_files(files)
                    .map_err(|e| ClipboardError::Set(format!("{e}")))?,
            }
            Ok(())
        })
        .await
        .map_err(|e| ClipboardError::Access(format!("Task join error: {e}")))?
    }

    /// Get current clipboard text for diagnostics and legacy tests.
    pub async fn get(&self) -> Result<String, ClipboardError> {
        let clipboard_arc = self.clipboard.clone();

        spawn_blocking(move || {
            let mut clipboard_guard = clipboard_arc.lock().unwrap();
            let clipboard = match clipboard_guard.as_mut() {
                Some(c) => c,
                None => {
                    let c = ClipboardContext::new()
                        .map_err(|e| ClipboardError::Access(format!("{e}")))?;
                    *clipboard_guard = Some(c);
                    clipboard_guard.as_mut().expect("clipboard inserted")
                }
            };

            clipboard
                .get_text()
                .map_err(|e| ClipboardError::Access(format!("{e}")))
        })
        .await
        .map_err(|e| ClipboardError::Access(format!("Task join error: {e}")))?
    }
}
