use anyhow::{Context, Result};
use clap::Parser;
use std::{future::IntoFuture, net::SocketAddr, path::PathBuf, time::Duration};
use thesauros::{AppState, config::Config};

#[derive(Parser)]
#[command(about = "Thesauros — a treasury for RAW photographs", version)]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    data: Option<PathBuf>,
    #[arg(long)]
    import_root: Vec<PathBuf>,
    #[arg(long)]
    listen: Option<SocketAddr>,
    #[arg(long)]
    public_url: Option<String>,
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// Validate configuration and credentials without starting the service.
    #[arg(long)]
    check_config: bool,
}
fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("THESAUROS_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .json()
        .with_writer(std::io::stderr)
        .init();
    if aigle_isolate::is_worker() {
        let memory = std::env::var("THESAUROS_PARSER_MEMORY_BYTES")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(2 * 1024 * 1024 * 1024);
        let old = rustix::process::getrlimit(rustix::process::Resource::As);
        let memory = old.maximum.map_or(memory, |max| max.min(memory));
        rustix::process::setrlimit(
            rustix::process::Resource::As,
            rustix::process::Rlimit {
                current: Some(memory),
                maximum: Some(memory),
            },
        )?;
        aigle_isolate::serve(&thesauros::worker_ops());
    }
    let args = Args::parse();
    let mut config = Config::load(args.config.as_deref())?;
    if let Some(data) = args.data {
        config.data = data;
    }
    if !args.import_root.is_empty() {
        config.import_roots = args.import_root;
    }
    if let Some(listen) = args.listen {
        config.listen = listen;
    }
    if let Some(url) = args.public_url {
        config.public_url = Some(url);
    }
    if let Some(file) = args.token_file {
        config.token_file = Some(file);
    }
    config.validate()?;
    let token = config.token()?;
    if args.check_config {
        println!("Thesauros configuration is valid");
        return Ok(());
    }
    anyhow::ensure!(
        aigle_isolate::enable(
            std::env::current_exe()?,
            vec![(
                "THESAUROS_PARSER_MEMORY_BYTES".into(),
                config.parser_memory_bytes.to_string().into()
            )]
        ),
        "Parser isolation is mandatory; remove AIGLE_ISOLATE=0"
    );
    let state = AppState::with_config(config, token)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                .clamp(2, 8),
        )
        .max_blocking_threads(64)
        .enable_all()
        .build()?;
    let result = runtime.block_on(run(state));
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}
async fn run(state: AppState) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(state.config.listen)
        .await
        .context("bind server listener")?;
    let address = listener.local_addr()?;
    let workers = thesauros::workers::start(state.clone());
    tracing::info!(%address,workers=state.config.index_workers,"Thesauros started");
    // Kept as a stable discovery line for local tooling; structured logs go to stderr.
    println!("Thesauros: http://{address}");
    let signals = state.clone();
    let signal_task = tokio::spawn(async move {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("register SIGTERM handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
        tracing::info!("shutdown requested");
        signals.stopping.cancel();
    });
    let shutdown = state.stopping.clone();
    let serving = axum::serve(
        listener,
        thesauros::router(state.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { shutdown.cancelled().await })
    .into_future();
    tokio::pin!(serving);
    let mut drain_deadline = None;
    let result = tokio::select! {
        result=&mut serving=>result.context("HTTP server failed"),
        _=state.stopping.cancelled()=>{
            let deadline=tokio::time::Instant::now()+Duration::from_secs(state.config.shutdown_seconds);drain_deadline=Some(deadline);
            match tokio::time::timeout_at(deadline,&mut serving).await {
                Ok(result)=>result.context("HTTP shutdown failed"),
                Err(_)=>{tracing::warn!("HTTP drain deadline reached");Ok(())}
            }
        }
    };
    state.stopping.cancel();
    signal_task.abort();
    let deadline = drain_deadline.unwrap_or_else(|| {
        tokio::time::Instant::now() + Duration::from_secs(state.config.shutdown_seconds)
    });
    for worker in workers {
        if tokio::time::timeout_at(deadline, worker).await.is_err() {
            tracing::warn!(
                "index worker drain deadline reached; persisted jobs will resume on restart"
            );
            break;
        }
    }
    tracing::info!("Thesauros stopped");
    result
}
