//! Updates of an installed PeetCAD, from the releases on GitHub.
//!
//! A release is packaged with [Velopack](https://velopack.io) (see
//! `.github/workflows/release.yml`): its `Setup.exe` installs PeetCAD for the current user,
//! and the same release holds the packages an installed copy updates itself from. An
//! [`Updater`] looks for a newer release and downloads it in the background; the update is
//! put in place by Velopack's updater once PeetCAD has closed, which then starts it again.
//! An update that was downloaded but not installed is installed the next time PeetCAD
//! starts ([`startup`]).
//!
//! A copy that was not installed (built from source, or the bare `.exe`) and the web build
//! have nothing to update: their status is [`Status::NotInstalled`] and stays so.

/// Where the releases are. Setting the environment variable `PEETCAD_UPDATE_SOURCE` to
/// another repository, a URL or a folder of packages takes them from there (for testing a
/// release before it is published).
pub const RELEASES: &str = "https://github.com/billpeet/peetcad";

/// How far an [`Updater`] is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// This copy was not installed, so it can't be updated.
    NotInstalled,
    /// Nothing was looked for yet.
    Idle,
    /// Asking what the newest release is.
    Checking,
    /// This is the newest release.
    UpToDate,
    /// A newer release is being downloaded.
    Downloading { version: String, percent: u8 },
    /// A newer release is downloaded: it is installed when PeetCAD restarts.
    Ready { version: String },
    /// Looking or downloading went wrong (no connection, say). Checking again may work.
    Failed(String),
}

impl Status {
    /// Whether a check is running.
    pub fn busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading { .. })
    }
}

/// Does what the installer asks of the application when it starts: call this first in
/// `main`. It finishes an update that was downloaded earlier (restarting into the new
/// version), and answers the installer's and uninstaller's calls, which end the process.
/// In a copy that was not installed, and on the web, it does nothing.
pub fn startup() {
    #[cfg(not(target_arch = "wasm32"))]
    velopack::VelopackApp::build().run();
}

/// Looks for updates and downloads them, off the UI thread. Read [`Updater::status`] each
/// frame; `wake` is called whenever it changed.
pub struct Updater {
    #[cfg(not(target_arch = "wasm32"))]
    installed: Option<native::Installed>,
}

impl Updater {
    /// The updater of this copy of PeetCAD. Nothing is looked for until [`Updater::check`].
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                installed: native::Installed::locate(Box::new(wake)),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = wake;
            Self {}
        }
    }

    pub fn status(&self) -> Status {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(installed) = &self.installed {
            return installed.status();
        }
        Status::NotInstalled
    }

    /// Looks for a newer release and downloads it, in the background. Does nothing while a
    /// check is running, or if this copy was not installed.
    pub fn check(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(installed) = &self.installed {
            installed.check();
        }
    }

    /// Starts the installer of the downloaded update. It waits (up to a minute) for
    /// PeetCAD to close, installs the update and starts PeetCAD again: close the
    /// application straight after this returns `Ok`.
    pub fn install_on_exit(&self) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(installed) = &self.installed {
            return installed.install_on_exit();
        }
        Err("This copy of PeetCAD was not installed, so it can't be updated.".to_owned())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Arc, Mutex, PoisonError, mpsc};

    use velopack::{UpdateCheck, UpdateManager, sources::AutoSource};

    use super::{RELEASES, Status};

    struct Shared {
        status: Mutex<Status>,
        wake: Box<dyn Fn() + Send + Sync>,
    }

    impl Shared {
        fn get(&self) -> Status {
            self.status
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn set(&self, status: Status) {
            *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
            (self.wake)();
        }

        /// Marks a check as started, unless one is running.
        fn begin(&self) -> bool {
            let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
            if status.busy() {
                return false;
            }
            *status = Status::Checking;
            true
        }
    }

    pub(super) struct Installed {
        manager: UpdateManager,
        shared: Arc<Shared>,
    }

    impl Installed {
        /// The installation this process runs from, if it is one.
        pub(super) fn locate(wake: Box<dyn Fn() + Send + Sync>) -> Option<Self> {
            let source = std::env::var("PEETCAD_UPDATE_SOURCE")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| RELEASES.to_owned());
            let manager = match UpdateManager::new(AutoSource::new(&source), None, None) {
                Ok(manager) => manager,
                Err(e) => {
                    log::info!("No updates: {e}");
                    return None;
                }
            };
            log::info!(
                "Installed as {} {}, updating from {source}",
                manager.get_app_id(),
                manager.get_current_version_as_string()
            );
            let status = ready(&manager).unwrap_or(Status::Idle);
            Some(Self {
                manager,
                shared: Arc::new(Shared {
                    status: Mutex::new(status),
                    wake,
                }),
            })
        }

        pub(super) fn status(&self) -> Status {
            self.shared.get()
        }

        pub(super) fn check(&self) {
            if !self.shared.begin() {
                return;
            }
            (self.shared.wake)();
            let (manager, shared) = (self.manager.clone(), self.shared.clone());
            let spawned = std::thread::Builder::new()
                .name("updates".to_owned())
                .spawn(move || {
                    let status = look(&manager, &shared);
                    log::info!("Updates: {status:?}");
                    shared.set(status);
                });
            if let Err(e) = spawned {
                self.shared
                    .set(Status::Failed(format!("Couldn't start looking: {e}")));
            }
        }

        pub(super) fn install_on_exit(&self) -> Result<(), String> {
            let Some(update) = self.manager.get_update_pending_restart() else {
                return Err("No update is downloaded yet.".to_owned());
            };
            self.manager
                .wait_exit_then_apply_updates(&update, false, true, Vec::<String>::new())
                .map_err(|e| format!("The update couldn't be installed: {e}"))
        }
    }

    /// The update that is downloaded and waits for a restart, if there is one.
    fn ready(manager: &UpdateManager) -> Option<Status> {
        manager
            .get_update_pending_restart()
            .map(|update| Status::Ready {
                version: update.Version,
            })
    }

    /// Asks for the newest release and downloads it if it is newer than this one.
    fn look(manager: &UpdateManager, shared: &Shared) -> Status {
        let update = match manager.check_for_updates() {
            Ok(UpdateCheck::UpdateAvailable(update)) => update,
            Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => {
                return ready(manager).unwrap_or(Status::UpToDate);
            }
            // An update downloaded earlier is still there to install.
            Err(e) => {
                return ready(manager)
                    .unwrap_or_else(|| Status::Failed(format!("Couldn't look for updates: {e}")));
            }
        };
        let version = update.TargetFullRelease.Version.clone();
        let downloading = |percent: i16| Status::Downloading {
            version: version.clone(),
            percent: u8::try_from(percent.clamp(0, 100)).unwrap_or(0),
        };
        shared.set(downloading(0));
        let (progress, percents) = mpsc::channel();
        let downloaded = std::thread::scope(|scope| {
            // Ends when the download does: it drops the sender.
            scope.spawn(|| {
                for percent in percents {
                    shared.set(downloading(percent));
                }
            });
            manager.download_updates(&update, Some(progress))
        });
        match downloaded {
            Ok(()) => Status::Ready { version },
            Err(e) => Status::Failed(format!("Couldn't download PeetCAD {version}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test binary is not an installed PeetCAD.
    #[test]
    fn a_copy_that_was_not_installed_has_nothing_to_update() {
        let updater = Updater::new(|| panic!("nothing changes"));
        assert_eq!(updater.status(), Status::NotInstalled);
        updater.check();
        assert_eq!(updater.status(), Status::NotInstalled);
        assert!(updater.install_on_exit().is_err());
    }

    #[test]
    fn a_check_is_busy_until_it_ends() {
        assert!(Status::Checking.busy());
        assert!(
            Status::Downloading {
                version: "1.2.3".to_owned(),
                percent: 40
            }
            .busy()
        );
        for done in [
            Status::NotInstalled,
            Status::Idle,
            Status::UpToDate,
            Status::Ready {
                version: "1.2.3".to_owned(),
            },
            Status::Failed("no connection".to_owned()),
        ] {
            assert!(!done.busy(), "{done:?}");
        }
    }
}
