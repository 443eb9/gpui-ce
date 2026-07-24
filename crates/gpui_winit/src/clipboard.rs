use std::{borrow::Cow, cell::RefCell, io::Cursor, path::PathBuf};

use anyhow::{Context, Result, anyhow, ensure};
use arboard::{Clipboard, Error as ClipboardError, ImageData};
use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, Image, ImageFormat};

pub(crate) struct WinitClipboard {
    clipboard: RefCell<Option<Clipboard>>,
}

impl WinitClipboard {
    pub(crate) fn new() -> Self {
        Self {
            clipboard: RefCell::new(None),
        }
    }

    pub(crate) fn read(&self) -> Option<ClipboardItem> {
        let mut clipboard = self.clipboard.borrow_mut();
        let clipboard = Self::get_or_create(&mut clipboard)?;

        match clipboard.get().file_list() {
            Ok(paths) if !paths.is_empty() => {
                return Some(ClipboardItem::from(ClipboardEntry::ExternalPaths(
                    ExternalPaths(paths.into_iter().collect()),
                )));
            }
            Ok(_) | Err(ClipboardError::ContentNotAvailable) => {}
            Err(ClipboardError::ClipboardOccupied) => {
                log::warn!("system clipboard is occupied");
                return None;
            }
            Err(error) => log::debug!("failed to read clipboard paths: {error}"),
        }

        let mut entries = Vec::new();
        match clipboard.get_text() {
            Ok(text) => entries.push(ClipboardEntry::String(gpui::ClipboardString::new(text))),
            Err(ClipboardError::ContentNotAvailable) => {}
            Err(ClipboardError::ClipboardOccupied) => {
                log::warn!("system clipboard is occupied");
                return None;
            }
            Err(error) => log::debug!("failed to read clipboard text: {error}"),
        }

        match clipboard.get_image() {
            Ok(image) => match encode_png(image) {
                Ok(image) => entries.push(ClipboardEntry::Image(image)),
                Err(error) => log::warn!("failed to encode clipboard image: {error}"),
            },
            Err(ClipboardError::ContentNotAvailable) => {}
            Err(ClipboardError::ClipboardOccupied) => {
                log::warn!("system clipboard is occupied");
            }
            Err(error) => log::debug!("failed to read clipboard image: {error}"),
        }

        (!entries.is_empty()).then_some(ClipboardItem { entries })
    }

    pub(crate) fn write(&self, item: ClipboardItem) {
        let mut text = String::new();
        let mut has_text = false;
        let mut image = None;
        let mut paths = Vec::<PathBuf>::new();

        for entry in item.into_entries() {
            match entry {
                ClipboardEntry::String(string) => {
                    has_text = true;
                    text.push_str(&string.text);
                }
                ClipboardEntry::Image(entry) if image.is_none() => image = Some(entry),
                ClipboardEntry::Image(_) => {}
                ClipboardEntry::ExternalPaths(entry) => paths.extend(entry.0),
            }
        }

        let mut clipboard = self.clipboard.borrow_mut();
        let Some(clipboard) = Self::get_or_create(&mut clipboard) else {
            return;
        };

        if !paths.is_empty() {
            match clipboard.set().file_list(&paths) {
                Ok(()) => return,
                Err(error) => log::warn!("failed to write clipboard paths: {error}"),
            }
            if !has_text {
                text = paths
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n");
                has_text = true;
            }
        }

        if let Some(image) = image {
            match decode_image(&image).and_then(|image| {
                clipboard
                    .set_image(image)
                    .map_err(|error| anyhow!(error.to_string()))
            }) {
                Ok(()) => return,
                Err(error) => {
                    log::warn!("failed to write clipboard image: {error}");
                    if !has_text {
                        return;
                    }
                }
            }
        }

        if has_text {
            if let Err(error) = clipboard.set_text(text) {
                log::warn!("failed to write clipboard text: {error}");
            }
        } else if let Err(error) = clipboard.clear() {
            log::warn!("failed to clear clipboard: {error}");
        }
    }

    fn get_or_create(clipboard: &mut Option<Clipboard>) -> Option<&mut Clipboard> {
        if clipboard.is_none() {
            match Clipboard::new() {
                Ok(new_clipboard) => *clipboard = Some(new_clipboard),
                Err(error) => {
                    log::warn!("failed to initialize system clipboard: {error}");
                    return None;
                }
            }
        }
        clipboard.as_mut()
    }
}

fn encode_png(image: ImageData<'static>) -> Result<Image> {
    let width = u32::try_from(image.width).context("clipboard image width is too large")?;
    let height = u32::try_from(image.height).context("clipboard image height is too large")?;
    let expected_len = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("clipboard image dimensions overflow")?;
    ensure!(
        image.bytes.len() == expected_len,
        "clipboard image has invalid RGBA data"
    );
    let rgba = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .context("clipboard image has invalid dimensions")?;
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .context("failed to encode PNG")?;
    Ok(Image::from_bytes(ImageFormat::Png, bytes.into_inner()))
}

fn decode_image(image: &Image) -> Result<ImageData<'static>> {
    let format = match image.format() {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Ico => image::ImageFormat::Ico,
        ImageFormat::Pnm => image::ImageFormat::Pnm,
        ImageFormat::Svg => return Err(anyhow!("SVG clipboard images are not supported")),
    };
    let rgba = image::load_from_memory_with_format(image.bytes(), format)
        .context("failed to decode clipboard image")?
        .into_rgba8();
    Ok(ImageData {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        bytes: Cow::Owned(rgba.into_raw()),
    })
}
