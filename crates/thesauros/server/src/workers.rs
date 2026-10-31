use crate::AppState;
use std::time::Duration;
use tokio::task::JoinHandle;

pub fn start(state: AppState) -> Vec<JoinHandle<()>> {
    let mut tasks = Vec::new();
    for worker in 0..state.config.index_workers {
        let state = state.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                if state.stopping.is_cancelled() {break}
                match state.call(|lib|lib.claim()).await {
                    Ok(Some(job))=> {
                        tracing::info!(worker,job_id=%job.id,kind=%job.kind,"job started");
                        let id=job.id.clone();
                        if let Err(error)=state.call(move|lib|lib.run_job(&job)).await {tracing::error!(worker,job_id=%id,error=?error,"job processing failed");}
                    },
                    Ok(None)=> {tokio::select!{ _=state.stopping.cancelled()=>break,_=state.wake.notified()=>{},_=tokio::time::sleep(Duration::from_secs(2))=>{} }},
                    Err(error)=> {tracing::error!(worker,error=?error,"job claim failed");tokio::time::sleep(Duration::from_secs(2)).await;}
                }
            }
        }));
    }
    let housekeeping = state.clone();
    tasks.push(tokio::spawn(async move {
        loop {tokio::select!{_=housekeeping.stopping.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(3600))=>{if let Err(error)=housekeeping.call(|lib|lib.housekeeping()).await{tracing::error!(error=?error,"housekeeping failed");}}}}
    }));
    tasks
}
