//! The React build, embedded in the binary. Unknown paths fall back to `index.html` so client
//! routes load directly; `/sw.js` is served from the root so the service worker covers the app.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Web;

fn file(path: &str) -> Option<Response> {
    let asset = Web::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut res = Response::new(Body::from(asset.data.into_owned()));
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref()).ok()?,
    );
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Some(res)
}

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if !path.is_empty()
        && let Some(res) = file(path)
    {
        return res;
    }
    // Client routes can end in a file name (`/w/{ws}/f/{key}/src/main.rs`), so only asset-shaped
    // paths 404: the build's `assets/` folder and root-level files like `/favicon.ico`.
    if path.starts_with("api/")
        || path.starts_with("internal/")
        || path.starts_with("assets/")
        || !path.contains('/') && path.contains('.') && !path.ends_with(".html")
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    file("index.html").unwrap_or_else(|| {
        (
            StatusCode::OK,
            "The web build is missing. Run `npm run build` in web/ and rebuild ostra.",
        )
            .into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn status(path: &str) -> StatusCode {
        static_handler(path.parse().unwrap()).await.status()
    }

    #[tokio::test]
    async fn client_routes_load_the_app_and_missing_assets_404() {
        assert_eq!(status("/w/ws_1/settings").await, StatusCode::OK);
        assert_eq!(status("/w/ws_1/f/app/src/main.rs").await, StatusCode::OK);
        assert_eq!(status("/assets/gone-1234.js").await, StatusCode::NOT_FOUND);
        assert_eq!(status("/favicon.ico").await, StatusCode::NOT_FOUND);
        assert_eq!(status("/api/nope").await, StatusCode::NOT_FOUND);
    }
}
