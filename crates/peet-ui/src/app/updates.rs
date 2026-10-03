//! Updates in the application: an installed PeetCAD looks for a newer release when it
//! starts, downloads it while the user works, and offers to restart once it is there.
//! Nothing is installed under a running PeetCAD: the update goes in when it restarts, or
//! the next time it starts (`peet_platform::update`).

use egui::Ui;
use peet_platform::update::{Status, Updater};
use peet_platform::{Duration, Instant};
use serde_json::{Value, json};

use super::{PeetApp, VERSION};
use crate::commands::CommandId;
use crate::files::AfterDiscard;

/// A PeetCAD left open looks again this often.
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);

/// Asked again this soon after it looked, PeetCAD says what it found then. (Asking is how
/// a script follows a check, and GitHub answers a computer only so often in an hour.)
const FRESH: Duration = Duration::from_secs(60);

pub(super) struct Updates {
    /// `None` until the application has a window (and in tests).
    updater: Option<Updater>,
    /// The updater's status, as last seen.
    status: Status,
    /// The window offering to restart is open.
    offer: bool,
    /// The version last offered, so looking again doesn't offer it again.
    offered: Option<String>,
    /// The user asked for the check that is running, so they are told how it ends.
    asked: bool,
    checked: Instant,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            updater: None,
            status: Status::NotInstalled,
            offer: false,
            offered: None,
            asked: false,
            checked: Instant::now(),
        }
    }
}

impl Updates {
    /// The version that is downloaded and waits for a restart.
    pub(super) fn ready(&self) -> Option<&str> {
        match &self.status {
            Status::Ready { version } => Some(version),
            _ => None,
        }
    }

    /// Whether looking for updates can start now.
    pub(super) fn can_check(&self) -> bool {
        self.status != Status::NotInstalled && !self.status.busy()
    }

    /// The status in a sentence, for the About window.
    pub(super) fn describe(&self) -> String {
        match &self.status {
            Status::NotInstalled => {
                "This copy was not installed, so it doesn't update itself.".to_owned()
            }
            Status::Idle => "Not looked for yet.".to_owned(),
            Status::Checking => "Looking for updates…".to_owned(),
            Status::UpToDate => "PeetCAD is up to date.".to_owned(),
            Status::Downloading { version, percent } => {
                format!("Downloading PeetCAD {version} ({percent}%)…")
            }
            Status::Ready { version } => {
                format!("PeetCAD {version} is downloaded: restart to install it.")
            }
            Status::Failed(e) => e.clone(),
        }
    }

    pub(super) fn install_on_exit(&self) -> Result<(), String> {
        match &self.updater {
            Some(updater) => updater.install_on_exit(),
            None => Err("No update is downloaded.".to_owned()),
        }
    }

    /// The update's place in the status bar (laid out right to left). Returns whether the
    /// user asked to restart.
    pub(super) fn status_bar(&self, ui: &mut Ui) -> bool {
        let mut restart = false;
        match &self.status {
            Status::Ready { version } => {
                restart = ui
                    .link(format!("Restart to update to {version}"))
                    .on_hover_text("PeetCAD closes, installs the update and starts again.")
                    .clicked();
                ui.separator();
            }
            Status::Downloading { version, percent } => {
                ui.weak(format!("Downloading PeetCAD {version}: {percent}%"));
                ui.separator();
            }
            _ => {}
        }
        restart
    }
}

impl PeetApp {
    /// Starts looking for updates, if this copy was installed and the user wants it to.
    pub(super) fn start_updates(&mut self, ctx: &egui::Context) {
        if peet_platform::is_web() {
            return;
        }
        let ctx = ctx.clone();
        let updater = Updater::new(move || ctx.request_repaint());
        self.updates.status = updater.status();
        self.updates.updater = Some(updater);
        if self.settings.check_for_updates {
            self.check_for_updates(false);
        }
    }

    /// Looks for a newer PeetCAD and downloads it, in the background. `asked`: the user
    /// asked for it, so they are told what comes of it.
    pub(super) fn check_for_updates(&mut self, asked: bool) {
        let fresh = self.updates.checked.elapsed() < FRESH;
        match &self.updates.status {
            Status::NotInstalled => {
                self.updates.checked = Instant::now();
                if asked {
                    self.info(self.updates.describe());
                }
                return;
            }
            Status::Ready { .. } if asked && fresh => {
                self.updates.offer = true;
                return;
            }
            Status::UpToDate if asked && fresh => {
                self.info(self.updates.describe());
                return;
            }
            status if status.busy() => {
                // The check that is running is now one the user waits for.
                self.updates.asked |= asked;
                return;
            }
            _ => {}
        }
        let Some(updater) = &self.updates.updater else {
            return;
        };
        self.updates.checked = Instant::now();
        updater.check();
        self.updates.status = updater.status();
        self.updates.asked = asked;
        if asked {
            self.info("Looking for updates…");
        }
    }

    /// Follows the updater: call once a frame.
    pub(super) fn poll_updates(&mut self) {
        if self.settings.check_for_updates && self.updates.checked.elapsed() >= RECHECK {
            self.check_for_updates(false);
        }
        let Some(updater) = &self.updates.updater else {
            return;
        };
        let status = updater.status();
        if status != self.updates.status {
            self.update_changed(status);
        }
    }

    /// The updater got further: offers a downloaded update, and says how a check the user
    /// asked for ended. One that PeetCAD started itself fails quietly (no connection is
    /// nothing to interrupt work for).
    fn update_changed(&mut self, status: Status) {
        let asked = self.updates.asked;
        match &status {
            Status::Ready { version } => {
                if asked || self.updates.offered.as_deref() != Some(version) {
                    self.updates.offer = true;
                    self.updates.offered = Some(version.clone());
                }
            }
            Status::UpToDate if asked => self.info("PeetCAD is up to date."),
            Status::Failed(e) if asked => self.error(e.clone()),
            _ => {}
        }
        if !status.busy() {
            self.updates.asked = false;
        }
        self.updates.status = status;
    }

    /// Restarts PeetCAD to install the downloaded update, asking about unsaved changes
    /// first.
    pub(super) fn install_update(&mut self) -> Result<(), String> {
        if self.updates.ready().is_none() {
            return Err(match &self.updates.status {
                Status::NotInstalled => self.updates.describe(),
                status if status.busy() => {
                    "The update is still being downloaded: ask again with check_for_updates."
                        .to_owned()
                }
                _ => "No update is downloaded: check_for_updates looks for one.".to_owned(),
            });
        }
        self.updates.offer = false;
        self.guard_unsaved(AfterDiscard::InstallUpdate);
        Ok(())
    }

    /// The window that offers to restart once an update is downloaded.
    pub(super) fn update_window(&mut self, ctx: &egui::Context, pending: &mut Vec<CommandId>) {
        if !self.updates.offer {
            return;
        }
        let Some(version) = self.updates.ready().map(str::to_owned) else {
            self.updates.offer = false;
            return;
        };
        let mut restart = None;
        egui::Window::new("Update Ready")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "PeetCAD {version} is downloaded. You have {VERSION}."
                ));
                ui.weak("Restarting installs it. Otherwise it is installed the next time PeetCAD starts.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Restart Now").clicked() {
                        restart = Some(true);
                    }
                    if ui.button("Later").clicked() {
                        restart = Some(false);
                    }
                });
            });
        match restart {
            Some(true) => pending.push(CommandId::InstallUpdate),
            Some(false) => self.updates.offer = false,
            None => {}
        }
    }

    /// How far the update is, for the reply to an operation.
    pub(super) fn update_json(&self) -> Value {
        let mut out = json!({ "current": VERSION });
        let (word, version) = match &self.updates.status {
            Status::NotInstalled => ("not_installed", None),
            Status::Idle => ("idle", None),
            Status::Checking => ("checking", None),
            Status::UpToDate => ("up_to_date", None),
            Status::Downloading { version, percent } => {
                out["percent"] = json!(percent);
                ("downloading", Some(version))
            }
            Status::Ready { version } => ("ready", Some(version)),
            Status::Failed(e) => {
                out["error"] = json!(e);
                ("failed", None)
            }
        };
        out["status"] = json!(word);
        if let Some(version) = version {
            out["version"] = json!(version);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_ops::{Undo, parse};

    fn ready(version: &str) -> Status {
        Status::Ready {
            version: version.to_owned(),
        }
    }

    fn apply(app: &mut PeetApp, op: Value) -> Value {
        let ctx = egui::Context::default();
        let op = parse(&app.doc, &op).unwrap();
        app.apply_op(&ctx, &op, Undo::Step).json
    }

    #[test]
    fn a_downloaded_update_is_offered_once() {
        let mut app = PeetApp::headless();
        app.update_changed(Status::Checking);
        app.update_changed(Status::Downloading {
            version: "0.2.0".to_owned(),
            percent: 50,
        });
        assert!(!app.updates.offer);
        app.update_changed(ready("0.2.0"));
        assert!(app.updates.offer);
        assert_eq!(app.updates.ready(), Some("0.2.0"));

        // "Later": looking again finds the same version, and doesn't ask again.
        app.updates.offer = false;
        app.update_changed(Status::Checking);
        app.update_changed(ready("0.2.0"));
        assert!(!app.updates.offer);
        // A newer one is offered, and so is the same one when the user looked.
        app.update_changed(ready("0.3.0"));
        assert!(app.updates.offer);
        app.updates.offer = false;
        app.updates.asked = true;
        app.update_changed(ready("0.3.0"));
        assert!(app.updates.offer);
        assert!(!app.updates.asked);
    }

    #[test]
    fn only_a_check_the_user_asked_for_reports_how_it_ended() {
        let mut app = PeetApp::headless();
        app.update_changed(Status::Failed("No connection.".to_owned()));
        assert_eq!(app.status_message, None);
        app.updates.asked = true;
        app.update_changed(Status::Checking);
        assert!(app.updates.asked);
        app.update_changed(Status::Failed("No connection.".to_owned()));
        assert_eq!(
            app.status_message,
            Some(("No connection.".to_owned(), true))
        );
        app.updates.asked = true;
        app.update_changed(Status::UpToDate);
        assert_eq!(
            app.status_message,
            Some(("PeetCAD is up to date.".to_owned(), false))
        );
    }

    #[test]
    fn restarting_to_update_asks_about_unsaved_changes_first() {
        let mut app = PeetApp::headless();
        assert!(app.install_update().is_err(), "nothing is downloaded");
        apply(
            &mut app,
            json!({"op": "set_parameter", "name": "t", "value": 2}),
        );
        assert!(app.doc.is_modified());
        app.update_changed(ready("0.2.0"));
        app.install_update().unwrap();
        assert_eq!(app.files.confirm, Some(AfterDiscard::InstallUpdate));
        assert!(!app.updates.offer);
        assert!(!app.quit_requested);
    }

    #[test]
    fn operations_reach_the_updates() {
        let mut app = PeetApp::headless();
        let reply = apply(&mut app, json!({"op": "check_for_updates"}));
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["update"]["status"], "not_installed");
        assert_eq!(reply["update"]["current"], VERSION);

        let reply = apply(&mut app, json!({"op": "install_update"}));
        assert_eq!(reply["ok"], false);
        assert!(
            reply["error"].as_str().unwrap().contains("not installed"),
            "{reply}"
        );
        app.update_changed(Status::Idle);
        let reply = apply(&mut app, json!({"op": "install_update"}));
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("check_for_updates"),
            "{reply}"
        );
        // Asked again soon after it looked, it says what it found: that is how a script
        // follows a check.
        app.update_changed(ready("0.2.0"));
        app.updates.offer = false;
        let reply = apply(&mut app, json!({"op": "check_for_updates"}));
        assert_eq!(reply["update"]["status"], "ready");
        assert_eq!(reply["update"]["version"], "0.2.0");
        assert!(app.updates.offer, "and the user is offered it again");
        app.update_changed(Status::UpToDate);
        let reply = apply(&mut app, json!({"op": "check_for_updates"}));
        assert_eq!(reply["update"]["status"], "up_to_date");

        assert!(app.settings.check_for_updates);
        let reply = apply(
            &mut app,
            json!({"op": "toggle", "what": "automatic_updates", "on": false}),
        );
        assert_eq!(reply["automatic_updates"], false, "{reply}");
        assert!(!app.settings.check_for_updates);
    }
}
