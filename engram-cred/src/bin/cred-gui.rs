//! cred-gui -- egui frontend for the engram-credd credential daemon.
//!
//! Connects to engram-credd on CREDD_URL (default http://localhost:4400) using
//! CRED_OWNER_KEY for auth. All HTTP calls run on a background thread;
//! results are sent back to the UI via an mpsc channel.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// API types (matches engram-credd response shapes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct SecretListItem {
    service: String,
    key: String,
    secret_type: String,
    #[allow(dead_code)]
    created_at: String,
    #[allow(dead_code)]
    updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SecretValueRaw {
    #[allow(dead_code)]
    service: String,
    #[allow(dead_code)]
    key: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    secret_type: String,
    value: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct ListResponse {
    secrets: Vec<SecretListItem>,
}

#[derive(Debug, Serialize)]
struct StoreRequest {
    data: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Messages between background worker and UI
// ---------------------------------------------------------------------------

enum WorkerMsg {
    List(Vec<SecretListItem>),
    Revealed(String, String, String),
    Stored,
    Deleted(String, String),
    Error(String),
}

enum UiCmd {
    LoadList,
    Reveal(String, String),
    StoreApiKey(String, String, String),
    Delete(String, String),
}

// ---------------------------------------------------------------------------
// HTTP worker
// ---------------------------------------------------------------------------

fn spawn_worker(
    credd_url: String,
    owner_key: String,
    cmd_rx: Receiver<UiCmd>,
    msg_tx: Sender<WorkerMsg>,
) {
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::new();

        for cmd in cmd_rx {
            let result = match cmd {
                UiCmd::LoadList => {
                    match client
                        .get(format!("{}/secrets", credd_url))
                        .bearer_auth(&owner_key)
                        .send()
                    {
                        Ok(r) if r.status().is_success() => match r.json::<ListResponse>() {
                            Ok(resp) => WorkerMsg::List(resp.secrets),
                            Err(e) => WorkerMsg::Error(format!("parse error: {e}")),
                        },
                        Ok(r) => WorkerMsg::Error(format!("credd error: {}", r.status())),
                        Err(e) => WorkerMsg::Error(format!("connection failed: {e}")),
                    }
                }

                UiCmd::Reveal(service, key) => {
                    let url = format!(
                        "{}/secret/{}/{}",
                        credd_url,
                        urlencod(&service),
                        urlencod(&key)
                    );
                    match client.get(&url).bearer_auth(&owner_key).send() {
                        Ok(r) if r.status().is_success() => match r.json::<SecretValueRaw>() {
                            Ok(sv) => {
                                let display = value_to_display(&sv.value);
                                WorkerMsg::Revealed(service, key, display)
                            }
                            Err(e) => WorkerMsg::Error(format!("parse error: {e}")),
                        },
                        Ok(r) => WorkerMsg::Error(format!("reveal failed: {}", r.status())),
                        Err(e) => WorkerMsg::Error(format!("connection failed: {e}")),
                    }
                }

                UiCmd::StoreApiKey(service, key, api_key_val) => {
                    let body = StoreRequest {
                        data: serde_json::json!({
                            "type": "api_key",
                            "key": api_key_val
                        }),
                    };
                    let url = format!(
                        "{}/secret/{}/{}",
                        credd_url,
                        urlencod(&service),
                        urlencod(&key)
                    );
                    match client.post(&url).bearer_auth(&owner_key).json(&body).send() {
                        Ok(r) if r.status().is_success() || r.status().as_u16() == 201 => {
                            WorkerMsg::Stored
                        }
                        Ok(r) => WorkerMsg::Error(format!("store failed: {}", r.status())),
                        Err(e) => WorkerMsg::Error(format!("connection failed: {e}")),
                    }
                }

                UiCmd::Delete(service, key) => {
                    let url = format!(
                        "{}/secret/{}/{}",
                        credd_url,
                        urlencod(&service),
                        urlencod(&key)
                    );
                    match client.delete(&url).bearer_auth(&owner_key).send() {
                        Ok(r) if r.status().is_success() || r.status().as_u16() == 204 => {
                            WorkerMsg::Deleted(service, key)
                        }
                        Ok(r) => WorkerMsg::Error(format!("delete failed: {}", r.status())),
                        Err(e) => WorkerMsg::Error(format!("connection failed: {e}")),
                    }
                }
            };

            let _ = msg_tx.send(result);
        }
    });
}

/// Extract a human-readable display string from a SecretData JSON value.
fn value_to_display(value: &serde_json::Value) -> String {
    if let Some(obj) = value.as_object() {
        let type_str = obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        match type_str {
            "api_key" => obj
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("[no key]")
                .to_string(),
            "login" => {
                let u = obj.get("username").and_then(|v| v.as_str()).unwrap_or("?");
                let p = obj.get("password").and_then(|v| v.as_str()).unwrap_or("?");
                format!("username={} password={}", u, p)
            }
            "note" => obj
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("[no content]")
                .to_string(),
            "oauth_app" => {
                let cid = obj.get("client_id").and_then(|v| v.as_str()).unwrap_or("?");
                format!("client_id={}", cid)
            }
            "ssh_key" => "[private key]".to_string(),
            "environment" => {
                if let Some(vars) = obj.get("variables").and_then(|v| v.as_object()) {
                    let names: Vec<String> = vars.keys().map(|k| format!("{}=***", k)).collect();
                    names.join(", ")
                } else {
                    "[env]".to_string()
                }
            }
            _ => serde_json::to_string(value).unwrap_or_else(|_| "[unparseable]".to_string()),
        }
    } else {
        value.to_string()
    }
}

/// Percent-encode a URL path segment.
fn urlencod(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// UI state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct AddDialog {
    open: bool,
    service: String,
    key: String,
    value: String,
    show_value: bool,
}

#[derive(Default)]
struct DeleteDialog {
    open: bool,
    service: String,
    key: String,
}

struct CredApp {
    cmd_tx: Sender<UiCmd>,
    msg_rx: Receiver<WorkerMsg>,
    secrets: Vec<SecretListItem>,
    revealed: HashMap<(String, String), String>,
    pending_reveal: HashSet<(String, String)>,
    filter: String,
    status: String,
    loading: bool,
    add: AddDialog,
    delete: DeleteDialog,
}

impl CredApp {
    fn new(cmd_tx: Sender<UiCmd>, msg_rx: Receiver<WorkerMsg>) -> Self {
        let _ = cmd_tx.send(UiCmd::LoadList);
        Self {
            cmd_tx,
            msg_rx,
            secrets: Vec::new(),
            revealed: HashMap::new(),
            pending_reveal: HashSet::new(),
            filter: String::new(),
            status: "Loading...".to_string(),
            loading: true,
            add: AddDialog::default(),
            delete: DeleteDialog::default(),
        }
    }

    fn process_messages(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            self.loading = false;
            match msg {
                WorkerMsg::List(items) => {
                    self.secrets = items;
                    self.status = format!("{} secrets", self.secrets.len());
                }
                WorkerMsg::Revealed(service, key, value) => {
                    self.pending_reveal.remove(&(service.clone(), key.clone()));
                    self.revealed.insert((service, key), value);
                }
                WorkerMsg::Stored => {
                    self.status = "Stored.".to_string();
                    self.add = AddDialog::default();
                    let _ = self.cmd_tx.send(UiCmd::LoadList);
                    self.loading = true;
                }
                WorkerMsg::Deleted(service, key) => {
                    self.secrets
                        .retain(|s| !(s.service == service && s.key == key));
                    self.revealed.remove(&(service.clone(), key.clone()));
                    self.status = format!("Deleted {}/{}", service, key);
                }
                WorkerMsg::Error(e) => {
                    self.status = format!("Error: {}", e);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// egui rendering
// ---------------------------------------------------------------------------

impl eframe::App for CredApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_messages();

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("cred vault");
                ui.separator();
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("+ Add").clicked() {
                        self.add.open = true;
                    }
                    if ui.button("Refresh").clicked() && !self.loading {
                        let _ = self.cmd_tx.send(UiCmd::LoadList);
                        self.loading = true;
                        self.status = "Loading...".to_string();
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut self.filter);
                if ui.small_button("x").clicked() {
                    self.filter.clear();
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.loading && self.secrets.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                });
                return;
            }

            let filter_lower = self.filter.to_lowercase();
            let mut grouped: HashMap<String, Vec<&SecretListItem>> = HashMap::new();
            for s in &self.secrets {
                if filter_lower.is_empty()
                    || s.service.to_lowercase().contains(&filter_lower)
                    || s.key.to_lowercase().contains(&filter_lower)
                {
                    grouped.entry(s.service.clone()).or_default().push(s);
                }
            }

            if grouped.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(if self.filter.is_empty() {
                        "No secrets stored."
                    } else {
                        "No matches."
                    });
                });
                return;
            }

            let mut services: Vec<String> = grouped.keys().cloned().collect();
            services.sort();

            egui::ScrollArea::vertical().show(ui, |ui| {
                for service in &services {
                    let entries = &grouped[service];
                    egui::CollapsingHeader::new(egui::RichText::new(service).strong())
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Grid::new(service)
                                .num_columns(4)
                                .striped(true)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    for entry in entries.iter() {
                                        let rk = (entry.service.clone(), entry.key.clone());

                                        ui.label(&entry.key);
                                        ui.label(
                                            egui::RichText::new(&entry.secret_type)
                                                .color(egui::Color32::from_rgb(150, 150, 200))
                                                .small(),
                                        );

                                        if let Some(plain) = self.revealed.get(&rk) {
                                            let display = if plain.len() > 60 {
                                                format!("{}...", &plain[..60])
                                            } else {
                                                plain.clone()
                                            };
                                            ui.monospace(&display);
                                            if ui.small_button("hide").clicked() {
                                                self.revealed.remove(&rk);
                                            }
                                        } else if self.pending_reveal.contains(&rk) {
                                            ui.spinner();
                                            ui.label("");
                                        } else {
                                            ui.label(&entry.secret_type);
                                            if ui.small_button("reveal").clicked() {
                                                self.pending_reveal.insert(rk.clone());
                                                let _ = self.cmd_tx.send(UiCmd::Reveal(
                                                    entry.service.clone(),
                                                    entry.key.clone(),
                                                ));
                                            }
                                        }

                                        if ui
                                            .small_button(
                                                egui::RichText::new("del")
                                                    .color(egui::Color32::from_rgb(200, 80, 80)),
                                            )
                                            .clicked()
                                        {
                                            self.delete = DeleteDialog {
                                                open: true,
                                                service: entry.service.clone(),
                                                key: entry.key.clone(),
                                            };
                                        }

                                        ui.end_row();
                                    }
                                });
                        });
                    ui.add_space(4.0);
                }
            });
        });

        // Add dialog
        if self.add.open {
            egui::Window::new("Add Secret (api-key type)")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    egui::Grid::new("add_grid").num_columns(2).show(ui, |ui| {
                        ui.label("Service:");
                        ui.text_edit_singleline(&mut self.add.service);
                        ui.end_row();
                        ui.label("Key:");
                        ui.text_edit_singleline(&mut self.add.key);
                        ui.end_row();
                        ui.label("Value:");
                        if self.add.show_value {
                            ui.text_edit_singleline(&mut self.add.value);
                        } else {
                            ui.add(egui::TextEdit::singleline(&mut self.add.value).password(true));
                        }
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.add.show_value, "show value");
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        let can_store = !self.add.service.is_empty()
                            && !self.add.key.is_empty()
                            && !self.add.value.is_empty();
                        if ui
                            .add_enabled(can_store, egui::Button::new("Store"))
                            .clicked()
                        {
                            let _ = self.cmd_tx.send(UiCmd::StoreApiKey(
                                self.add.service.clone(),
                                self.add.key.clone(),
                                self.add.value.clone(),
                            ));
                            self.status = "Storing...".to_string();
                            self.loading = true;
                        }
                        if ui.button("Cancel").clicked() {
                            self.add = AddDialog::default();
                        }
                    });
                });
        }

        // Delete confirmation
        if self.delete.open {
            let service = self.delete.service.clone();
            let key = self.delete.key.clone();
            egui::Window::new("Confirm Delete")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Delete {}/{}?", service, key));
                    ui.label(
                        egui::RichText::new("This cannot be undone.")
                            .color(egui::Color32::from_rgb(200, 80, 80)),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                egui::RichText::new("Delete")
                                    .color(egui::Color32::from_rgb(200, 80, 80)),
                            )
                            .clicked()
                        {
                            let _ = self.cmd_tx.send(UiCmd::Delete(service, key));
                            self.delete = DeleteDialog::default();
                        }
                        if ui.button("Cancel").clicked() {
                            self.delete = DeleteDialog::default();
                        }
                    });
                });
        }

        if self.loading || !self.pending_reveal.is_empty() {
            ctx.request_repaint();
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let credd_url =
        std::env::var("CREDD_URL").unwrap_or_else(|_| "http://localhost:4400".to_string());

    let owner_key = match std::env::var("CRED_OWNER_KEY") {
        Ok(k) => k,
        Err(_) => {
            eprintln!("error: CRED_OWNER_KEY env var not set");
            eprintln!("  export CRED_OWNER_KEY=<your key>");
            std::process::exit(1);
        }
    };

    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCmd>();
    let (msg_tx, msg_rx) = mpsc::channel::<WorkerMsg>();

    spawn_worker(credd_url, owner_key, cmd_rx, msg_tx);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("cred vault")
            .with_inner_size([800.0, 560.0])
            .with_min_inner_size([500.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        "cred vault",
        options,
        Box::new(|_cc| Ok(Box::new(CredApp::new(cmd_tx, msg_rx)))),
    )
    .expect("failed to launch cred-gui");
}
