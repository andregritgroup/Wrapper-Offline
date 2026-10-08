mod ai;
mod lineart;
mod model;
mod pdf;
mod store;
mod story;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::model::{Project, Slot};
use crate::store::{NotFound, Store};

#[derive(Clone)]
struct AppState {
    store: Arc<Store>,
    http: reqwest::Client,
    settings: Arc<RwLock<ai::Settings>>,
}

struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = if self.0.downcast_ref::<NotFound>().is_some() {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_GATEWAY
        };
        let msg = format!("{:#}", self.0);
        eprintln!("error: {msg}");
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

type ApiResult<T> = Result<T, AppError>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = std::env::var("CBS_DATA_DIR").unwrap_or_else(|_| "library".into());
    let host: std::net::IpAddr = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into()).parse()?;
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);

    let store = Store::open(&data_dir)?;
    let mut settings = store.load_settings()?;
    if settings.hf_token.is_empty() {
        settings.hf_token = std::env::var("HF_TOKEN").unwrap_or_default();
    }
    if settings.runware_token.is_empty() {
        settings.runware_token = std::env::var("RUNWARE_API_KEY").unwrap_or_default();
    }
    if settings.pollinations_token.is_empty() {
        settings.pollinations_token = std::env::var("POLLINATIONS_TOKEN").unwrap_or_default();
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent(concat!("coloring-book-studio/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let state = AppState { store: Arc::new(store), http, settings: Arc::new(RwLock::new(settings)) };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/:id", get(get_project).put(update_project).delete(delete_project))
        .route("/api/projects/:id/split", post(split_pages))
        .route("/api/projects/:id/duplicate", post(duplicate_project))
        .route("/api/projects/:id/slots/:slot/prompt", get(slot_prompt))
        .route("/api/projects/:id/slots/:slot/generate", post(generate_slot))
        .route("/api/projects/:id/slots/:slot/upload", post(upload_slot))
        .route("/api/projects/:id/images/:file", get(get_image))
        .route("/api/projects/:id/book.pdf", get(get_pdf))
        .layer(DefaultBodyLimit::max(40 * 1024 * 1024))
        .with_state(state.clone());

    let addr = SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Colouring Book Studio is running: open http://{addr} in your browser");
    println!("Books are saved in {}", state.store.root().display());
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn get_settings(State(st): State<AppState>) -> Json<ai::PublicSettings> {
    Json(st.settings.read().await.public())
}

async fn put_settings(State(st): State<AppState>, Json(incoming): Json<ai::Settings>) -> ApiResult<Json<ai::PublicSettings>> {
    let mut s = st.settings.write().await;
    s.merge(incoming);
    st.store.save_settings(&s)?;
    Ok(Json(s.public()))
}

async fn list_projects(State(st): State<AppState>) -> ApiResult<impl IntoResponse> {
    Ok(Json(st.store.list()?))
}

#[derive(Deserialize, Default)]
struct CreateBody {
    title: Option<String>,
}

async fn create_project(State(st): State<AppState>, body: Option<Json<CreateBody>>) -> ApiResult<Json<Project>> {
    let title = body.and_then(|b| b.0.title);
    Ok(Json(st.store.create(title)?))
}

async fn get_project(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Project>> {
    Ok(Json(st.store.load(&id)?))
}

async fn update_project(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(p): Json<Project>,
) -> ApiResult<Json<Project>> {
    Ok(Json(st.store.update(&id, p)?))
}

async fn delete_project(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    st.store.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn duplicate_project(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Project>> {
    Ok(Json(st.store.duplicate(&id)?))
}

#[derive(Deserialize)]
struct SplitBody {
    target_pages: Option<usize>,
}

async fn split_pages(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SplitBody>,
) -> ApiResult<Json<Project>> {
    let p = st.store.load(&id)?;
    let pages = story::split_story(&p.story, p.age_band, body.target_pages)
        .iter()
        .map(|page| story::extract_picture(page))
        .filter(|(text, scene)| !text.is_empty() || !scene.is_empty())
        .collect();
    Ok(Json(st.store.replace_pages(&id, pages)?))
}

async fn slot_prompt(State(st): State<AppState>, Path((id, slot)): Path<(String, String)>) -> ApiResult<impl IntoResponse> {
    let p = st.store.load(&id)?;
    let ip = story::image_prompt(&p, &Slot::parse(&slot)).ok_or_else(|| NotFound(format!("page {slot:?}")))?;
    Ok(Json(serde_json::json!({
        "prompt": ip.prompt, "negative": ip.negative, "width": ip.width, "height": ip.height, "seed": ip.seed
    })))
}

async fn generate_slot(State(st): State<AppState>, Path((id, slot)): Path<(String, String)>) -> ApiResult<Json<Project>> {
    let slot = Slot::parse(&slot);
    if slot == Slot::Style {
        return Err(anyhow::anyhow!("the style picture is uploaded, not generated").into());
    }
    let p = st.store.load(&id)?;
    let req = story::image_prompt(&p, &slot).ok_or_else(|| NotFound("page".into()))?;
    let settings = st.settings.read().await.clone();
    let reference = if settings.provider.uses_references() && !req.references.is_empty() {
        let mut images = Vec::new();
        for name in &req.references {
            images.push(image::open(st.store.image_path(&id, name)?)?.to_luma8());
        }
        Some(tokio::task::spawn_blocking(move || lineart::reference_sheet(&images)).await??)
    } else {
        None
    };
    let raw = ai::generate(&st.http, &settings, &req, reference).await?;
    let age = p.detail_level();
    let png = tokio::task::spawn_blocking(move || lineart::to_coloring_png(&raw, age)).await??;
    Ok(Json(st.store.set_image(&id, &slot, &png, true)?))
}

/// Upload your own drawing or photo of a sketch; it gets the same clean-up as AI images.
async fn upload_slot(
    State(st): State<AppState>,
    Path((id, slot)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult<Json<Project>> {
    let slot = Slot::parse(&slot);
    let p = st.store.load(&id)?;
    let age = p.detail_level();
    let png = tokio::task::spawn_blocking(move || lineart::to_coloring_png(&body, age))
        .await?
        .map_err(|e| anyhow::anyhow!("that file is not an image I can read ({e})"))?;
    Ok(Json(st.store.set_image(&id, &slot, &png, false)?))
}

async fn get_image(State(st): State<AppState>, Path((id, file)): Path<(String, String)>) -> ApiResult<Response> {
    let bytes = tokio::fs::read(st.store.image_path(&id, &file)?).await?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=31536000, immutable")], bytes).into_response())
}

async fn get_pdf(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let p = st.store.load(&id)?;
    let store = st.store.clone();
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let mut images = HashMap::new();
        let names = p.pages.iter().filter_map(|pg| pg.image.clone()).chain(p.cover_image.clone());
        for name in names {
            let path = store.image_path(&p.id, &name)?;
            images.insert(name, image::open(path)?.to_luma8());
        }
        Ok(pdf::build_book(&p, &images))
    })
    .await??;
    let p = st.store.load(&id)?;
    let file: String = p
        .title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let file = if file.is_empty() { "colouring-book".to_string() } else { file };
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{file}.pdf\"")),
        ],
        bytes,
    )
        .into_response())
}
