//! The running application as a session `peet` can reach: operations that arrive over
//! the local socket (`peet_live`) are applied here, between frames, as if the user had
//! done them.

use peet_live::Server;
use peet_ops::{Reply, Undo};
use peet_platform::Instant;
use serde_json::Value;

use super::{PeetApp, VERSION};

/// Operations are applied for this long in one frame; the rest wait for the next, so the
/// window keeps drawing through a long script.
const FRAME_BUDGET_MS: f64 = 40.0;

impl PeetApp {
    /// Starts listening for `peet`. Without it the application works as before, so a
    /// failure is only logged.
    pub(super) fn start_live(&mut self, ctx: &egui::Context) {
        let Some(dir) = peet_live::sessions_dir() else {
            return;
        };
        let ctx = ctx.clone();
        match Server::start(&dir, VERSION, move || ctx.request_repaint()) {
            Ok(server) => {
                log::info!("Listening for peet as {}", server.session().socket);
                self.live = Some(server);
            }
            Err(e) => log::warn!("Not listening for peet: {e}"),
        }
    }

    /// Applies the operations `peet` sent since the last frame.
    pub(super) fn serve_live(&mut self, ctx: &egui::Context) {
        let Some(server) = self.live.take() else {
            return;
        };
        self.serve(ctx, &server);
        self.live = Some(server);
    }

    pub(super) fn serve(&mut self, ctx: &egui::Context, server: &Server) {
        let start = Instant::now();
        let mut applied = 0;
        while peet_platform::elapsed_ms(start) < FRAME_BUDGET_MS {
            let Some(request) = server.next() else {
                break;
            };
            let reply = self.apply_json(ctx, request.op());
            request.reply(reply);
            applied += 1;
        }
        if applied > 0 {
            // There may be more waiting, and the part is to be drawn as it is now.
            ctx.request_repaint();
        }
        let file = self.doc.file.as_ref().and_then(|f| f.path.as_deref());
        server.set_document(file, &self.doc.title(), self.doc.is_modified());
    }

    /// Applies an operation written as JSON, and says in the status bar what was done.
    fn apply_json(&mut self, ctx: &egui::Context, op: &Value) -> Value {
        let name = op["op"].as_str().unwrap_or_default().to_owned();
        let reply = match peet_ops::parse(&self.doc, op) {
            Ok(op) => self.apply_op(ctx, &op, Undo::Step),
            Err(e) => Reply::error(&name, e),
        };
        if reply.changed || reply.replaced {
            let label = self.doc.undo_label().map(str::to_owned);
            self.info(match label.filter(|_| reply.changed) {
                Some(label) => format!("peet: {label}"),
                None => format!("peet: {name}"),
            });
        }
        reply.json
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_live::Client;
    use serde_json::json;

    #[test]
    fn operations_sent_to_the_session_change_the_open_part() {
        let dir = std::env::temp_dir().join(format!("peet-ui-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        let server = Server::start(&dir, VERSION, || {}).unwrap();
        app.serve(&ctx, &server);

        // What peet does, from another thread: find the session and send a script.
        let folder = dir.clone();
        let peet = std::thread::spawn(move || {
            let sessions = peet_live::sessions(&folder);
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].file, None);
            let mut client = Client::connect(&sessions[0]).unwrap();
            let mut send = |op: Value| client.send(&op).unwrap();
            let sketch = send(json!({"op": "sketch", "on": "top", "draw": [
                {"type": "rectangle", "from": [0, 0], "to": [60, 40]},
            ]}));
            let extrude = send(json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}));
            let wrong = send(json!({"op": "extrude", "sketch": "Nothing", "depth": 5}));
            let unknown = send(json!({"op": "extrood"}));
            let grid = send(json!({"op": "toggle", "what": "grid", "on": false}));
            let bodies = send(json!({"op": "bodies"}));
            // While a sketch is open in the application the part can't be changed.
            let editing = send(json!({"op": "edit_sketch", "sketch": "Sketch1"}));
            let busy = send(json!({"op": "set_parameter", "name": "t", "value": 2}));
            let asked = send(json!({"op": "features"}));
            let done = send(json!({"op": "exit_sketch"}));
            send(json!({"op": "status", "last": true}));
            [
                sketch, extrude, wrong, unknown, grid, bodies, editing, busy, asked, done,
            ]
        });
        // The application's frames.
        while !peet.is_finished() {
            app.serve(&ctx, &server);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let [
            sketch,
            extrude,
            wrong,
            unknown,
            grid,
            bodies,
            editing,
            busy,
            asked,
            done,
        ] = peet.join().unwrap();

        assert_eq!(sketch["ok"], true, "{sketch}");
        assert_eq!(extrude["created"][0]["name"], "Extrude1");
        assert_eq!(wrong["ok"], false);
        assert!(
            wrong["error"].as_str().unwrap().contains("Nothing"),
            "{wrong}"
        );
        assert_eq!(unknown["ok"], false);
        assert_eq!(grid["ok"], true, "{grid}");
        assert!(!app.settings.show_grid);
        assert_eq!(bodies["bodies"][0]["size"], json!([60.0, 40.0, 5.0]));
        assert_eq!(editing["ok"], true, "{editing}");
        assert_eq!(busy["ok"], false);
        assert!(
            busy["error"].as_str().unwrap().contains("exit_sketch"),
            "{busy}"
        );
        assert_eq!(asked["ok"], true);
        assert_eq!(done["ok"], true, "{done}");

        // Each change is an undo step of the application, and the part is not saved.
        assert_eq!(app.doc.bodies.len(), 1);
        assert!(app.doc.can_undo());
        assert!(app.doc.is_modified());
        assert!(
            app.journal().len() >= 5,
            "what was applied is in the journal"
        );
        // The list says what is open, for the next peet.
        assert!(server.session().modified);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
