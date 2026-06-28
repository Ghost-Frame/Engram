//! Advanced (expert) configuration wizard step.
//!
//! Exposes the commonly-flipped server toggles as checkboxes plus a few value
//! fields, so the user does not have to hand-edit `kleos.toml` after install.
//! Only values changed from their default are emitted as overrides; everything
//! else stays at the server default (and the full surface remains reachable in
//! the generated `kleos.toml`). Open access lives on the Security step, so it
//! is intentionally not duplicated here.

use eframe::egui;
use kleos_install_core::config::ConfigOverrides;

use crate::theme;
use crate::wizard::InstallerApp;

/// Persistent state for the advanced toggles, edited in-place by egui.
///
/// Booleans are initialized to the server defaults so a comparison can decide
/// whether to emit an override. Value fields are empty until the user types,
/// meaning "leave at default".
#[derive(Debug, Clone)]
pub struct AdvancedToggles {
    /// Run the periodic auto-backup task.
    pub backup_enabled: bool,
    /// Run the background dreamer/consolidation loop.
    pub dreamer_enabled: bool,
    /// Let the dreamer auto-link unlinked memories each cycle.
    pub auto_link_enabled: bool,
    /// Maintain PageRank scores for connectivity-based boosting.
    pub pagerank_enabled: bool,
    /// Run autonomous skill evolution inside the dreamer tick.
    pub skill_evolution_enabled: bool,
    /// Auto-evaluate sessions when they end.
    pub thymus_autoeval_enabled: bool,
    /// Run the scheduled community-detection job.
    pub community_detection_enabled: bool,
    /// Enable consolidation endpoints (can degrade search; off by default).
    pub consolidation_enabled: bool,
    /// Add the structured-facts RRF retrieval channel (changes ranking).
    pub facts_channel_enabled: bool,
    /// Search at chunk granularity in addition to whole memories.
    pub use_chunk_vector_search: bool,
    /// Refuse to download model weights at boot (air-gapped).
    pub embedding_offline_only: bool,
    /// SearXNG base URL backing /search/web (empty = default).
    pub web_search_url: String,
    /// Pre-auth per-IP rate limit in requests/minute (empty = default).
    pub preauth_ip_rpm: String,
    /// GUI password; any non-empty value enables the web GUI.
    pub gui_password: String,
}

/// Default construction seeding toggles from the server defaults.
impl Default for AdvancedToggles {
    /// Initialize every toggle to its server default and value fields to empty.
    fn default() -> Self {
        AdvancedToggles {
            backup_enabled: false,
            dreamer_enabled: true,
            auto_link_enabled: true,
            pagerank_enabled: true,
            skill_evolution_enabled: true,
            thymus_autoeval_enabled: true,
            community_detection_enabled: false,
            consolidation_enabled: false,
            facts_channel_enabled: false,
            use_chunk_vector_search: false,
            embedding_offline_only: false,
            web_search_url: String::new(),
            preauth_ip_rpm: String::new(),
            gui_password: String::new(),
        }
    }
}

/// Conversion of the advanced toggles into installer overrides.
impl AdvancedToggles {
    /// Fold the toggles into [`ConfigOverrides`]: booleans that differ from the
    /// default become `toml_overrides`, non-empty values become overrides or env
    /// entries, and unchanged settings are omitted so the config stays minimal.
    pub fn to_overrides(&self) -> ConfigOverrides {
        let defaults = AdvancedToggles::default();
        let mut overrides = ConfigOverrides::default();

        // Emit a bool override only when it differs from the server default.
        let mut push_bool = |key: &str, cur: bool, def: bool| {
            if cur != def {
                overrides
                    .toml_overrides
                    .push((key.to_string(), cur.to_string()));
            }
        };
        push_bool(
            "backup_enabled",
            self.backup_enabled,
            defaults.backup_enabled,
        );
        push_bool(
            "dreamer_enabled",
            self.dreamer_enabled,
            defaults.dreamer_enabled,
        );
        push_bool(
            "auto_link_enabled",
            self.auto_link_enabled,
            defaults.auto_link_enabled,
        );
        push_bool(
            "pagerank_enabled",
            self.pagerank_enabled,
            defaults.pagerank_enabled,
        );
        push_bool(
            "skill_evolution_enabled",
            self.skill_evolution_enabled,
            defaults.skill_evolution_enabled,
        );
        push_bool(
            "thymus_autoeval_enabled",
            self.thymus_autoeval_enabled,
            defaults.thymus_autoeval_enabled,
        );
        push_bool(
            "community_detection_enabled",
            self.community_detection_enabled,
            defaults.community_detection_enabled,
        );
        push_bool(
            "consolidation_enabled",
            self.consolidation_enabled,
            defaults.consolidation_enabled,
        );
        push_bool(
            "facts_channel_enabled",
            self.facts_channel_enabled,
            defaults.facts_channel_enabled,
        );
        push_bool(
            "use_chunk_vector_search",
            self.use_chunk_vector_search,
            defaults.use_chunk_vector_search,
        );
        push_bool(
            "embedding_offline_only",
            self.embedding_offline_only,
            defaults.embedding_offline_only,
        );

        let web = self.web_search_url.trim();
        if !web.is_empty() {
            overrides
                .toml_overrides
                .push(("web_search_url".to_string(), web.to_string()));
        }
        let rpm = self.preauth_ip_rpm.trim();
        if !rpm.is_empty() {
            overrides
                .toml_overrides
                .push(("preauth_ip_rpm".to_string(), rpm.to_string()));
        }
        let pw = self.gui_password.trim();
        if !pw.is_empty() {
            overrides
                .extra_env
                .push(("KLEOS_GUI_PASSWORD".to_string(), pw.to_string()));
        }

        overrides
    }
}

/// Draw the advanced configuration step.
pub fn draw_advanced(ui: &mut egui::Ui, app: &mut InstallerApp) {
    ui.heading("Advanced Settings");
    ui.add_space(4.0);
    ui.colored_label(
        theme::COLOR_TEXT_DIM,
        "Optional. Every other setting is written to kleos.toml at its default for later editing.",
    );
    ui.add_space(12.0);

    let adv = &mut app.advanced;

    ui.label("Background workers");
    ui.checkbox(&mut adv.backup_enabled, "Auto-backup the database");
    ui.checkbox(&mut adv.dreamer_enabled, "Dreamer maintenance task");
    ui.checkbox(&mut adv.auto_link_enabled, "Associative auto-linker");
    ui.checkbox(&mut adv.pagerank_enabled, "PageRank scoring");
    ui.checkbox(&mut adv.skill_evolution_enabled, "Skill evolution");
    ui.checkbox(&mut adv.thymus_autoeval_enabled, "Thymus auto-evaluation");
    ui.checkbox(
        &mut adv.community_detection_enabled,
        "Community detection (scheduled)",
    );
    ui.checkbox(
        &mut adv.consolidation_enabled,
        "Consolidation endpoints (can degrade search)",
    );
    ui.add_space(8.0);

    ui.label("Retrieval");
    ui.checkbox(
        &mut adv.facts_channel_enabled,
        "Facts retrieval channel (changes ranking)",
    );
    ui.checkbox(
        &mut adv.use_chunk_vector_search,
        "Chunk-level vector search",
    );
    ui.add_space(8.0);

    ui.label("Embedding");
    ui.checkbox(
        &mut adv.embedding_offline_only,
        "Offline-only (no model downloads at boot)",
    );
    ui.add_space(8.0);

    ui.label("Web search (SearXNG) URL:");
    ui.add(egui::TextEdit::singleline(&mut adv.web_search_url).hint_text("http://127.0.0.1:8888"));
    ui.add_space(8.0);

    ui.label("Pre-auth per-IP rate limit (requests/minute):");
    ui.add(egui::TextEdit::singleline(&mut adv.preauth_ip_rpm).hint_text("60"));
    ui.add_space(8.0);

    ui.label("GUI password (enables the web GUI):");
    ui.add(
        egui::TextEdit::singleline(&mut adv.gui_password)
            .password(true)
            .hint_text("leave blank to keep the GUI disabled"),
    );
}
