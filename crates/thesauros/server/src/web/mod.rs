mod actions;
pub use actions::router as actions;

use crate::{
    AppState,
    auth::Session,
    library::{FilterOptions, Job, Photo, Search},
};
use std::sync::LazyLock;
use topcoat::{
    context::{Cx, app_context},
    router::{page, parse_query_params, request, response::response_headers},
    view::{View, component, view},
};

pub static CSS_URL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "/assets/app-{}.css",
        &blake3::hash(include_bytes!("../../assets/app.css")).to_hex()[..16]
    )
});
pub static JS_URL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "/assets/app-{}.js",
        &blake3::hash(include_bytes!("../../assets/app.js")).to_hex()[..16]
    )
});
pub static ICON_URL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "/assets/icon-{}.svg",
        &blake3::hash(include_bytes!("../../assets/icon.svg")).to_hex()[..16]
    )
});
async fn session(cx: &Cx) -> topcoat::Result<Session> {
    let state = app_context::<AppState>(cx);
    state
        .session(request::headers(cx))
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| topcoat::router::error::unauthorized().into())
}
#[page("/")]
pub async fn home(cx: &Cx) -> topcoat::Result<impl View> {
    let state = app_context::<AppState>(cx);
    let session = state
        .session(request::headers(cx))
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let search: Search = parse_query_params(cx)
        .map_err(|_| topcoat::router::error::bad_request("Invalid search"))?;
    let photos = if session.is_some() {
        let q = search.clone();
        state
            .call(move |lib| lib.list(&q))
            .await
            .map_err(topcoat::Error::from_anyhow)?
    } else {
        vec![]
    };
    let jobs = if session.is_some() {
        state
            .call(|lib| lib.jobs())
            .await
            .map_err(topcoat::Error::from_anyhow)?
    } else {
        vec![]
    };
    let options = if session.is_some() {
        state
            .call(|lib| lib.filter_options())
            .await
            .map_err(topcoat::Error::from_anyhow)?
    } else {
        FilterOptions::default()
    };
    let authenticated = session.is_some();
    let login_error = request::uri(cx)
        .query()
        .is_some_and(|q| q == "login=failed");
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
          <head>
            <meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
            <meta name="htmx-config" content=r#"{"allowEval":false,"allowScriptTags":false,"includeIndicatorStyles":false,"selfRequestsOnly":true,"historyCacheSize":0}"#>
            <title>"Thesauros · RAW library"</title>
            <link rel="icon" href=(ICON_URL.as_str()) type="image/svg+xml">
            <link rel="stylesheet" href=(CSS_URL.as_str())>
            <script src="/assets/htmx-2.0.11.min.js" defer=""></script>
            <script src=(JS_URL.as_str()) defer=""></script>
          </head>
          <body class=(if authenticated{"app-page"}else{"auth-page"})>
            <a href="#main" class="sr-only focus:not-sr-only focus:fixed focus:left-4 focus:top-4 focus:z-50 focus:bg-surface focus:p-4">"Skip to content"</a>
            <header class="app-header">
              <div class="header-inner">
                <a href="/" class="brand">
                  <span class="brand-mark" aria-hidden="true">"Θ"</span>
                  <span>"Thesauros"</span>
                </a>
                if let Some(ref current) = session {
                  <form method="post" action="/actions/logout">
                    <input type="hidden" name="csrf" value=(&current.csrf)>
                    <button class="btn-ghost" type="submit">"Sign out"</button>
                  </form>
                }
              </div>
            </header>
            <main id="main" class=(if authenticated{"app-shell"}else{"auth-shell"})>
            if let Some(current) = session {
              <div class="workspace">
                <aside id="library-sidebar" class="sidebar" aria-label="Library tools">
                  <div class="sidebar-heading"><span>"Filters & tools"</span><button type="button" class="btn-ghost" data-sidebar-close="" aria-label="Close library tools">"×"</button></div>
                  filter_controls(options:options,search:search.clone())
                  <section class="tools-panel">
                    <form id="upload-form" method="post" action="/actions/upload" enctype="multipart/form-data" hx-post="/actions/upload" hx-encoding="multipart/form-data" hx-target="#notice" hx-swap="innerHTML" hx-disabled-elt="find button" class="flex flex-col gap-4">
                      <input type="hidden" name="csrf" value=(&current.csrf)>
                      <label class="field">"Batch upload"<input type="file" name="files" multiple="" required="" class="file-input"></label>
                      <button type="submit" class="btn w-full">"Upload RAW files"</button>
                      <progress max="100" value="0" aria-label="Upload progress" class="upload-progress"></progress>
                      <span class="htmx-indicator field-note">"Uploading…"</span>
                    </form>
                    <div class="tools-divider"></div>
                    <form method="post" action="/actions/import" hx-post="/actions/import" hx-target="#notice" hx-swap="innerHTML" hx-disabled-elt="find button" class="flex flex-col gap-3">
                      <input type="hidden" name="csrf" value=(&current.csrf)>
                      <label class="field">"Import from server"<input name="path" placeholder="/srv/photos" required=""></label>
                      <button type="submit" class="btn-secondary w-full">"Index folder"</button>
                      <p class="field-note">"Files stay in place."</p>
                    </form>
                  </section>
                  job_list(jobs: jobs, csrf: current.csrf.clone())
                </aside>
                <div class="workspace-content">
                  <div class="content-header"><h1 class="page-title">"Photos"</h1><button type="button" class="btn-secondary sidebar-toggle" aria-controls="library-sidebar" aria-expanded="false">"Filters & tools"</button></div>
                  <form id="search-form" method="get" action="/" hx-get="/ui/photos" hx-target="#gallery" hx-swap="outerHTML" hx-trigger="submit, input changed delay:350ms" hx-sync="this:replace" class="toolbar">
                    <div class="search-field"><label class="sr-only" for="search">"Search photos"</label>
                      <svg class="search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"></circle><path d="m16 16 4.5 4.5" stroke-linecap="round"></path></svg>
                      <input id="search" type="search" name="q" value=(&search.q) placeholder="Search photos…"></div>
                    <button type="submit" class="btn-secondary">"Search"</button>
                  </form>
                  <div id="notice" role="status" aria-live="polite" class="notice"></div>
                  gallery(photos: photos, search: search.clone(), csrf: current.csrf.clone())
                </div>
              </div>
              <button type="button" class="sidebar-backdrop" data-sidebar-close="" tabindex="-1" aria-label="Close library tools"></button>
            } else {
              <section class="signin-panel">
                <h1 class="mb-6 text-lg font-semibold text-zinc-100">"Sign in"</h1>
                if login_error {<p role="alert" class="mb-4 text-sm text-amber-300">"Invalid access token."</p>}
                <form method="post" action="/actions/login" class="space-y-5">
                  <label class="field">"Access token"<input name="token" type="password" autocomplete="current-password" required="" minlength="32" maxlength="1024"></label>
                  <button type="submit" class="btn w-full">"Sign in"</button>
                </form>
              </section>
            }
            </main>
          </body>
        </html>
    })
}
#[page("/ui/photos")]
pub async fn gallery_page(cx: &Cx) -> topcoat::Result<impl View> {
    let current = session(cx).await?;
    let state = app_context::<AppState>(cx);
    let search: Search = parse_query_params(cx)
        .map_err(|_| topcoat::router::error::bad_request("Invalid search"))?;
    let query = search.clone();
    let photos = state
        .call(move |lib| lib.list(&query))
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(view! {gallery(photos:photos,search:search,csrf:current.csrf)})
}
#[page("/ui/jobs")]
pub async fn jobs_page(cx: &Cx) -> topcoat::Result<impl View> {
    let current = session(cx).await?;
    let state = app_context::<AppState>(cx);
    let jobs = state
        .call(|lib| lib.jobs())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if topcoat::htmx::hx_request(cx) {
        response_headers(cx).append(
            "hx-trigger".parse().unwrap(),
            "libraryChanged".parse().unwrap(),
        );
    }
    Ok(view! {job_list(jobs:jobs,csrf:current.csrf)})
}
#[component]
async fn gallery(photos: Vec<Photo>, search: Search, csrf: String) -> topcoat::Result<impl View> {
    let count = photos.len();
    let limit = search.limit();
    let offset = search.offset();
    let empty_message = if offset > 0 {
        "No photos on this page"
    } else if !search.q.is_empty()
        || search.rating.is_some()
        || search.camera_value().is_some()
        || search.lens_value().is_some()
    {
        "No matching photos"
    } else {
        "No photos"
    };
    let pending = photos.iter().any(|p| p.status == "pending");
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("q", &search.q);
    if let Some(rating) = search.rating {
        query.append_pair("rating", &rating.to_string());
    }
    if let Some(camera) = search.camera_value() {
        query.append_pair("camera", camera);
    }
    if let Some(lens) = search.lens_value() {
        query.append_pair("lens", lens);
    }
    let query = query.finish();
    let refresh = format!("/ui/photos?{query}&offset={offset}&limit={limit}");
    let previous = format!(
        "/ui/photos?{query}&offset={}&limit={limit}",
        offset.saturating_sub(limit)
    );
    let next = format!("/ui/photos?{query}&offset={}&limit={limit}", offset + limit);
    Ok(view! {
      <section id="gallery" aria-label="Photographs" hx-get=(refresh) hx-trigger=(if pending{"every 3s, libraryChanged from:body"}else{"libraryChanged from:body"}) hx-swap="outerHTML" hx-sync="this:replace">
        if count>0 {<div class="gallery-meta"><p>(format!("{} {} · page {}",count,if count==1{"photo"}else{"photos"},offset/limit+1))</p><span>"Capture time ↓"</span></div>}
        if photos.is_empty() {
          <p class="empty-state">(empty_message)</p>
        } else {
          <div class="photo-grid">
          for photo in photos {
            <article class="photo-card">
              <div class="photo-preview">
                if photo.has_preview {<img src=(format!("/media/{}/preview",photo.id)) alt=(format!("Preview of {}",photo.name)) loading="lazy" decoding="async" class="h-full w-full object-contain">}
                else {<div class="preview-placeholder"><p>(if photo.status=="pending"{"Preparing preview…"}else{"No embedded preview"})</p></div>}
              </div>
              <div class="photo-info">
                <div class="photo-heading"><h2 class="photo-name" title=(&photo.name)>(&photo.name)</h2>if photo.status!="ready" {<span class=(format!("state state-{}",photo.status))>(&photo.status)</span>}</div>
                if let Some(camera)=photo.camera.as_ref() {<p class="camera-meta">(camera)</p>}
                if let Some(lens)=photo.lens.as_ref() {<p class="lens-meta">(lens)</p>}
                <div class="capture-meta">if let Some(captured)=photo.captured.as_ref() {<time>(captured)</time>}<span>(format!("{:.1} MiB",photo.size as f64/1048576.0))</span></div>
                if let Some(error)=photo.error.as_ref().filter(|error|error.as_str()!="No usable embedded preview") {<p class="photo-error">(error)</p>}
                <form method="post" action="/actions/rating" hx-post="/actions/rating" hx-target="#notice" hx-swap="innerHTML" hx-disabled-elt="find button" class="rating-form">
                  <input type="hidden" name="csrf" value=(&csrf)><input type="hidden" name="id" value=(&photo.id)><input type="hidden" name="revision" value=(&photo.revision)>
                  <select name="rating" aria-label=(format!("Rating for {}",photo.name)) class="min-w-0 flex-1">
                    for rating in -1i8..=5 {<option value=(rating) selected=(rating==photo.rating)>(if rating<0{"Rejected".into()}else if rating==0{"Unrated".to_string()}else{"★".repeat(rating as usize)})</option>}
                  </select><button type="submit" class="btn-secondary">"Save"</button>
                </form>
                <div class="photo-actions">
                  <a href=(format!("/media/{}/raw",photo.id)) class="text-action"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M12 3v12m-4-4 4 4 4-4M5 16v4h14v-4" stroke-linecap="round" stroke-linejoin="round"></path></svg>"Download RAW"</a>
                  <form method="post" action="/actions/reindex" hx-post="/actions/reindex" hx-target="#notice" hx-swap="innerHTML"><input type="hidden" name="csrf" value=(&csrf)><input type="hidden" name="id" value=(&photo.id)><button type="submit" class="text-action">"Reindex"</button></form>
                </div>
                <details class="photo-id"><summary>"Aigle photo ID"</summary><code>(&photo.id)</code></details>
              </div>
            </article>
          }
          </div>
        }
        if offset>0 || count==limit as usize {<nav class="mt-6 flex items-center justify-between gap-4" aria-label="Photo pages">
          if offset>0 {<a class="btn-secondary" href=(format!("/?{query}&offset={}&limit={limit}",offset.saturating_sub(limit))) hx-get=(previous) hx-target="#gallery" hx-swap="outerHTML">"← Previous"</a>}
          <span></span>
          if count==limit as usize {<a class="btn-secondary" href=(format!("/?{query}&offset={}&limit={limit}",offset+limit)) hx-get=(next) hx-target="#gallery" hx-swap="outerHTML">"Next →"</a>}
        </nav>}
      </section>
    })
}
#[component]
async fn job_list(jobs: Vec<Job>, csrf: String) -> topcoat::Result<impl View> {
    let active = jobs
        .iter()
        .any(|j| matches!(j.state.as_str(), "queued" | "running"));
    Ok(view! {
      <details id="jobs" class="panel jobs-panel" open=(active) aria-label="Background jobs" hx-get="/ui/jobs" hx-trigger=(if active{"every 3s, jobsChanged from:body"}else{"jobsChanged from:body"}) hx-swap="outerHTML" hx-sync="this:replace">
        <summary class="section-label">"Jobs"</summary><div class="mt-4">
        if jobs.is_empty() {<p class="field-note">"No jobs"</p>}
        <ul class="job-list">
        for job in jobs.into_iter().filter(|job|!active || matches!(job.state.as_str(),"queued"|"running"|"failed")).take(20) {
          <li class="job-item">
            <div class="job-heading"><span class="font-medium text-zinc-300">(if job.kind=="import"{"Folder import"}else{"Photo indexing"})</span><span class=(format!("state state-{}",job.state))>(&job.state)</span></div>
            if job.kind=="import" {<p class="mt-1 text-xs text-muted tabular-nums">(format!("{} processed · {} failed",job.processed,job.failed))</p>}
            if let Some(error)=job.error {<p class="mt-2 text-xs text-amber-300">(error)</p>}
            if matches!(job.state.as_str(),"queued"|"running"|"failed"|"cancelled") {
              <form method="post" action="/actions/job" hx-post="/actions/job" hx-target="#notice" hx-swap="innerHTML" class="mt-2">
                <input type="hidden" name="csrf" value=(&csrf)><input type="hidden" name="id" value=(&job.id)>
                <input type="hidden" name="operation" value=(if matches!(job.state.as_str(),"failed"|"cancelled"){"retry"}else{"cancel"})>
                <button type="submit" class="text-action">(if matches!(job.state.as_str(),"failed"|"cancelled"){"Retry"}else if job.cancel_requested{"Cancelling…"}else{"Cancel"})</button>
              </form>
            }
          </li>
        }
        </ul></div>
      </details>
    })
}

#[page("/ui/filters")]
pub async fn filters_page(cx: &Cx) -> topcoat::Result<impl View> {
    session(cx).await?;
    let state = app_context::<AppState>(cx);
    let search: Search = parse_query_params(cx)
        .map_err(|_| topcoat::router::error::bad_request("Invalid filters"))?;
    let options = state
        .call(|lib| lib.filter_options())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(view! {filter_controls(options:options,search:search)})
}
#[component]
async fn filter_controls(options: FilterOptions, search: Search) -> topcoat::Result<impl View> {
    let camera = search.camera_value().unwrap_or("").to_owned();
    let lens = search.lens_value().unwrap_or("").to_owned();
    Ok(view! {
      <section id="filters" class="sidebar-filters" aria-label="Photo filters" hx-get="/ui/filters" hx-trigger="libraryChanged from:body" hx-include="#search-form" hx-swap="outerHTML" hx-sync="this:replace">
        choice_picker(field:"camera",label:"Camera",all:"All cameras",options:options.cameras,selected:camera)
        choice_picker(field:"lens",label:"Lens",all:"All lenses",options:options.lenses,selected:lens)
        <label class="field" for="rating-filter">"Rating"</label>
        <select id="rating-filter" name="rating" form="search-form" data-auto-submit="" class="w-full">
          <option value="">"All ratings"</option>
          for rating in -1i8..=5 {<option value=(rating) selected=(search.rating==Some(rating))>(if rating<0{"Rejected".to_owned()}else if rating==0{"Unrated".to_owned()}else{format!("{rating} {}",if rating==1{"star"}else{"stars"})})</option>}
        </select>
      </section>
    })
}
#[component]
async fn choice_picker(
    field: &'static str,
    label: &'static str,
    all: &'static str,
    mut options: Vec<String>,
    selected: String,
) -> topcoat::Result<impl View> {
    if !selected.is_empty() && !options.contains(&selected) {
        options.push(selected.clone());
    }
    let value = if selected.is_empty() {
        all.to_owned()
    } else {
        selected.clone()
    };
    Ok(view! {
      <div id=(format!("{field}-picker")) class="choice-picker" data-picker=(field)>
        <label id=(format!("{field}-label")) for=(format!("{field}-select")) class="field">(label)</label>
        <select id=(format!("{field}-select")) name=(field) form="search-form" data-picker-native="" data-auto-submit="" class="w-full">
          <option value="" selected=(selected.is_empty())>(all)</option>
          for option in &options {<option value=(option) selected=(option==&selected)>(option)</option>}
        </select>
        <button type="button" class="picker-trigger" data-picker-trigger="" hidden="" aria-haspopup="listbox" aria-expanded="false" aria-controls=(format!("{field}-options")) aria-labelledby=(format!("{field}-label {field}-value"))>
          <span id=(format!("{field}-value")) data-picker-value="">(value)</span>
          <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" aria-hidden="true"><path d="m6 8 4 4 4-4" stroke-linecap="round" stroke-linejoin="round"></path></svg>
        </button>
        <div class="picker-menu" data-picker-menu=(format!("{field}-picker")) hidden="">
          <input type="search" role="combobox" aria-label=(format!("Filter {} options",label.to_lowercase())) aria-autocomplete="list" aria-controls=(format!("{field}-options")) aria-expanded="false" autocomplete="off" placeholder="Filter options…" data-picker-search="">
          <ul id=(format!("{field}-options")) role="listbox" aria-label=(format!("{label} options")) class="picker-options">
            <li id=(format!("{field}-option-all")) role="option" aria-label=(all) aria-selected=(if selected.is_empty(){"true"}else{"false"}) data-value="" class="picker-option">(all)</li>
            for (index,option) in options.iter().enumerate() {<li id=(format!("{field}-option-{index}")) role="option" aria-selected=(if option==&selected{"true"}else{"false"}) data-value=(option) class="picker-option">(option)</li>}
          </ul>
          <p class="picker-empty" data-picker-empty="" hidden="">"No matching options"</p>
        </div>
      </div>
    })
}
