//! Saving files the user asked for (exports).
//!
//! Natively this shows the system save dialog and writes the file. In the browser there is
//! no file system: the bytes are offered as a download with the suggested name.

/// Outcome of [`save_file`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Written to this location (a path natively, the download name on the web).
    Saved(String),
    /// The user closed the dialog.
    Cancelled,
}

/// Offers `bytes` to the user as a file called `suggested_name`. `filter` is a
/// description and the extensions to offer, like `("STL mesh", &["stl"])`.
pub fn save_file(
    suggested_name: &str,
    filter: (&str, &[&str]),
    bytes: &[u8],
) -> Result<SaveOutcome, String> {
    imp::save_file(suggested_name, filter, bytes)
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::SaveOutcome;

    pub fn save_file(
        suggested_name: &str,
        (description, extensions): (&str, &[&str]),
        bytes: &[u8],
    ) -> Result<SaveOutcome, String> {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(suggested_name)
            .add_filter(description, extensions)
            .save_file()
        else {
            return Ok(SaveOutcome::Cancelled);
        };
        std::fs::write(&path, bytes)
            .map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
        Ok(SaveOutcome::Saved(path.display().to_string()))
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::SaveOutcome;
    use wasm_bindgen::JsCast as _;

    pub fn save_file(
        suggested_name: &str,
        _filter: (&str, &[&str]),
        bytes: &[u8],
    ) -> Result<SaveOutcome, String> {
        let fail = |what: &str| format!("Couldn't start the download ({what}).");
        let window = web_sys::window().ok_or_else(|| fail("no window"))?;
        let document = window.document().ok_or_else(|| fail("no document"))?;
        let array = js_sys::Uint8Array::from(bytes);
        let parts = js_sys::Array::of1(&array);
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("application/octet-stream");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(|_| fail("blob"))?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| fail("url"))?;
        let anchor: web_sys::HtmlAnchorElement = document
            .create_element("a")
            .map_err(|_| fail("element"))?
            .dyn_into()
            .map_err(|_| fail("anchor"))?;
        anchor.set_href(&url);
        anchor.set_download(suggested_name);
        anchor.click();
        let _ = web_sys::Url::revoke_object_url(&url);
        Ok(SaveOutcome::Saved(suggested_name.to_owned()))
    }
}
