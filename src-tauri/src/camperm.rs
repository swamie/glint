//! Camera access for Glint's window, answered by Glint instead of WebView2's own prompt.
//!
//! A page in the window that opens a camera (the Stream plugin's preview) makes WebView2 raise
//! PermissionRequested. Left alone, WebView2 shows its browser prompt ("http://tauri.localhost
//! wants to use your cameras") and remembers the answer in its profile. Here the request is
//! answered at once from Glint's settings, so that prompt never shows:
//!   - camera: allowed when an installed plugin's settings say `camera_access: "allow"` (the user
//!     said yes in Glint's own popup, which the plugin shows before it first opens a camera);
//!   - microphone: always refused. Nothing in the window uses it (the capture's sound is not the page's).
//! Other kinds of request are not touched.
//!
//! The answer is not saved in WebView2's profile, so the setting stays the one place it lives, and
//! an answer once given to the browser's prompt is forgotten when the window opens.
use crate::core::Core;
use std::sync::Arc;
use tauri::{Runtime, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2PermissionRequestedEventArgs3, ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
    COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PERMISSION_STATE_DEFAULT, COREWEBVIEW2_PERMISSION_STATE_DENY,
};
use webview2_com::{PermissionRequestedEventHandler, SetPermissionStateCompletedHandler};
use windows_core_wv::{Interface, HSTRING};

/// The addresses Glint's own pages are served from.
const ORIGINS: [&str; 2] = ["http://tauri.localhost", "https://tauri.localhost"];

/// Take over the window's camera and microphone requests. Fails soft: on an older WebView2 the
/// browser's own prompt stays.
pub fn install<R: Runtime>(window: &WebviewWindow<R>, core: Arc<Core>) {
    let done = window.with_webview(move |wv| unsafe {
        let Ok(web) = wv.controller().CoreWebView2() else {
            return crate::util::log("camera: the window's WebView2 was not reachable");
        };
        // Forget what was once answered to the browser's own prompt.
        if let Ok(profile) = web.cast::<ICoreWebView2_13>().and_then(|w| w.Profile()).and_then(|p| p.cast::<ICoreWebView2Profile4>()) {
            for origin in ORIGINS {
                for kind in [COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE] {
                    let _ = profile.SetPermissionState(kind, &HSTRING::from(origin), COREWEBVIEW2_PERMISSION_STATE_DEFAULT, &SetPermissionStateCompletedHandler::create(Box::new(|_| Ok(()))));
                }
            }
        }
        let mut token = 0i64;
        let added = web.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                args.PermissionKind(&mut kind)?;
                if kind != COREWEBVIEW2_PERMISSION_KIND_CAMERA && kind != COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                    return Ok(());
                }
                let allow = kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA && crate::plugins::camera_allowed(&core.settings());
                args.SetState(if allow { COREWEBVIEW2_PERMISSION_STATE_ALLOW } else { COREWEBVIEW2_PERMISSION_STATE_DENY })?;
                if let Ok(a3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                    let _ = a3.SetSavesInProfile(false);
                }
                Ok(())
            })),
            &mut token,
        );
        if added.is_err() {
            crate::util::log("camera: could not take over the window's camera requests");
        }
    });
    if done.is_err() {
        crate::util::log("camera: the window's WebView2 was not reachable");
    }
}
