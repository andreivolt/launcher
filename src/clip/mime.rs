/// MIME detection from magic bytes using the imagesize crate.
///
/// Lightweight alternative to sniffing with the full `image` crate — imagesize
/// only reads headers, never decodes pixels.

use imagesize::ImageType;

/// Detect MIME type from the first bytes of content.
/// Returns a MIME string like "image/png" or falls back to "application/octet-stream".
pub fn detect_mime(data: &[u8]) -> &'static str {
    if data.is_empty() {
        return "application/octet-stream";
    }

    // Try imagesize first for image formats
    if let Ok(img_type) = imagesize::image_type(data) {
        return match img_type {
            ImageType::Png => "image/png",
            ImageType::Jpeg => "image/jpeg",
            ImageType::Gif => "image/gif",
            ImageType::Webp => "image/webp",
            ImageType::Bmp => "image/bmp",
            ImageType::Tiff => "image/tiff",
            ImageType::Ico => "image/x-icon",
            ImageType::Psd => "image/vnd.adobe.photoshop",
            ImageType::Jxl => "image/jxl",
            _ => "image/unknown",
        };
    }

    // Check if it's valid UTF-8 text
    if std::str::from_utf8(data).is_ok() {
        return "text/plain";
    }

    "application/octet-stream"
}

/// Returns true if the MIME type represents an image format we care about.
pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/unknown"
}

/// MIME negotiation: pick the best MIME type from a set of offered types.
///
/// Priority order:
/// 1. Image types (png > jpeg > gif > webp)
/// 2. UTF-8 text > plain text
/// 3. URI list
/// 4. Skip text/html when images are available
///
/// Returns None if no suitable type is found.
pub fn negotiate_mime(offered: &[String]) -> Option<&str> {
    // Priority-ordered image types
    const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

    let has_image = offered.iter().any(|m| IMAGE_TYPES.contains(&m.as_str()));

    // 1. Prefer images
    for img_type in IMAGE_TYPES {
        if offered.iter().any(|m| m == img_type) {
            return Some(img_type);
        }
    }

    // 2. UTF-8 text first, then plain text
    if offered.iter().any(|m| m == "text/plain;charset=utf-8") {
        return Some("text/plain;charset=utf-8");
    }
    if offered.iter().any(|m| m == "UTF8_STRING") {
        return Some("UTF8_STRING");
    }
    if offered.iter().any(|m| m == "text/plain") {
        return Some("text/plain");
    }
    if offered.iter().any(|m| m == "STRING") {
        return Some("STRING");
    }

    // 3. URI list
    if offered.iter().any(|m| m == "text/uri-list") {
        return Some("text/uri-list");
    }

    // 4. text/html only if no images were available
    if !has_image {
        if offered.iter().any(|m| m == "text/html") {
            return Some("text/html");
        }
    }

    None
}
