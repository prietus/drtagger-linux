//! Where the ONNX models live and a small pool of ONNX Runtime sessions
//! per model: a session runs one inference at a time, and the collector
//! scans three images at once.

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The models directory (`scripts/fetch-models.sh` fills it):
/// `$DRTAGGER_MODELS`, else `drtagger/models` under `$XDG_DATA_DIRS`
/// (`/app/share` in the Flatpak), else next to the executable
/// (`../share/drtagger/models`), else the source tree's `models/`.
pub fn models_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let mut candidates: Vec<PathBuf> = vec![];
        if let Some(d) = std::env::var_os("DRTAGGER_MODELS") {
            candidates.push(d.into());
        }
        let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        candidates.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("drtagger/models")));
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            candidates.push(exe.join("../share/drtagger/models"));
        }
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"));
        candidates.into_iter().find(|d| d.join("ocr/det.onnx").is_file())
    })
    .clone()
}

pub(crate) fn model(relative: &str) -> Option<PathBuf> {
    models_dir().map(|d| d.join(relative)).filter(|p| p.is_file())
}

/// Sessions of one model, created on demand and reused.
pub(crate) struct SessionPool {
    path: PathBuf,
    idle: Mutex<Vec<Session>>,
}

pub(crate) struct Lease<'a> {
    pool: &'a SessionPool,
    session: Option<Session>,
}

impl std::ops::Deref for Lease<'_> {
    type Target = Session;
    fn deref(&self) -> &Session {
        self.session.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for Lease<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        self.session.as_mut().unwrap()
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(s) = self.session.take() {
            self.pool.idle.lock().unwrap().push(s);
        }
    }
}

fn environment() -> &'static ort::environment::Environment {
    static ENV: OnceLock<ort::environment::Environment> = OnceLock::new();
    ENV.get_or_init(|| ort::init().with_name("drtagger").build().expect("ONNX Runtime environment"))
}

impl SessionPool {
    pub(crate) fn new(path: PathBuf) -> SessionPool {
        SessionPool { path, idle: Mutex::new(vec![]) }
    }

    /// Drops the sessions nobody is using (and the memory their arenas
    /// hold); the next lease builds one again.
    pub(crate) fn release_idle(&self) {
        self.idle.lock().unwrap().clear();
    }

    pub(crate) fn lease(&self) -> ort::Result<Lease<'_>> {
        let idle = self.idle.lock().unwrap().pop();
        let session = match idle {
            Some(s) => s,
            None => Session::builder(environment())?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(ort::Error::<()>::from)?
                .with_intra_threads(2)
                .map_err(ort::Error::<()>::from)?
                .commit_from_file(&self.path)?,
        };
        Ok(Lease { pool: self, session: Some(session) })
    }
}
