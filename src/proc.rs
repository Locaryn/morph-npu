//! Lancer `llama-server` de façon qu'il ne survive pas au lanceur.
//!
//! L'hôte arrête le lanceur ; un serveur resté en place garderait la mémoire
//! vidéo et le port, et le démarrage suivant échouerait sans explication.

use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Empêcher Windows d'ouvrir une fenêtre de console pour un sous-processus.
///
/// Le lanceur tourne lui-même sans console : tout programme console qu'il
/// démarre — `llama-server`, `nvidia-smi` — s'en voit alors allouer une neuve,
/// visible. Sans effet hors de Windows.
pub fn masquer_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Démarre le serveur, lié à la vie du lanceur.
pub fn spawn(serveur: &Path, args: &[String]) -> Result<Child, String> {
    let mut cmd = Command::new(serveur);
    cmd.args(args).stdin(Stdio::null());
    masquer_console(&mut cmd);
    // Les bibliothèques (CUDA, ggml) sont à côté de l'exécutable.
    if let Some(dossier) = serveur.parent() {
        cmd.current_dir(dossier);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: appel à prctl sans pointeur, dans le processus fils avant exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }
    let enfant = cmd
        .spawn()
        .map_err(|e| format!("impossible de lancer {} : {e}", serveur.display()))?;
    #[cfg(windows)]
    lier_au_lanceur(&enfant)?;
    Ok(enfant)
}

/// Un objet « job » qui tue ses processus quand son dernier handle se ferme,
/// c'est-à-dire quand le lanceur meurt, par quelque voie que ce soit.
#[cfg(windows)]
fn lier_au_lanceur(enfant: &Child) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // SAFETY: structures initialisées à zéro puis remplies ; le handle du job
    // est volontairement gardé ouvert jusqu'à la fin du processus.
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err("objet job indisponible".into());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            return Err("réglage de l'objet job refusé".into());
        }
        if AssignProcessToJobObject(job, enfant.as_raw_handle() as _) == 0 {
            return Err("rattachement du serveur au lanceur refusé".into());
        }
    }
    Ok(())
}
