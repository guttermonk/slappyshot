// What the user last reached for -- tool, size, colour -- kept across runs so
// the next capture opens the way they left this one.
//
// A file of its own, and not a corner of config.toml: that file is the user's,
// hand written and commented, and a program that rewrites it to store a colour
// is a program that eventually loses someone's comments. This one is the
// program's to overwrite freely. XDG has a name for exactly that -- state:
// generated, not essential, not a cache -- so it lives in $XDG_STATE_HOME
// (~/.local/state/slappyshot/state.toml) and nothing is lost if it is deleted.

use serde::{Deserialize, Serialize};

use crate::style::{Color, Size};
use crate::tools::ToolType;

const STATE_FILE: &str = "state.toml";

/// The remembered selection. Every field optional, because a state file
/// written by an older version -- or edited, or half written -- should
/// contribute what it has and leave the rest to the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LastUsed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    /// `#rrggbbaa`. A hex string rather than a Color so the file stays
    /// readable, and so a colour that is no longer in the palette can be
    /// carried about and compared without being honoured -- see App::new.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl LastUsed {
    /// Read the state file, falling back to "nothing remembered" for anything
    /// that goes wrong. There is no failure here worth interrupting a capture
    /// over: the worst case is a window that opens with the configured
    /// defaults, which is what every version before this one did.
    pub fn load() -> Self {
        let dirs = xdg::BaseDirectories::with_prefix("slappyshot");
        let Some(path) = dirs.get_state_file(STATE_FILE) else {
            return Self::default();
        };
        if !path.exists() {
            return Self::default();
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("Warning: failed to read {}: {}", path.display(), e);
                return Self::default();
            }
        };
        let mut state: Self = match toml::from_str(&text) {
            Ok(state) => state,
            Err(e) => {
                eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                return Self::default();
            }
        };
        // Normalised on the way in so that a hex string spelled some other
        // valid way -- uppercase, or the six digit form -- is not seen as a
        // change by App::remember_style and rewritten on the first frame.
        // An unparseable one is dropped rather than carried.
        state.color = state
            .color
            .as_deref()
            .and_then(Color::from_hex)
            .map(Color::to_hex);
        state
    }

    /// Write the state file, warning on failure and otherwise carrying on --
    /// a selection that cannot be stored is not worth a lost annotation.
    /// Called whenever the selection changes rather than at exit, because
    /// both ways out of this program are abrupt -- `std::process::exit` from
    /// the configured exit actions, and the compositor closing the window --
    /// and neither runs a shutdown hook.
    pub fn save(&self) {
        let dirs = xdg::BaseDirectories::with_prefix("slappyshot");
        let text = match toml::to_string(self) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("Warning: failed to serialize last-used state: {}", e);
                return;
            }
        };
        // Written beside the real file and renamed over it, so a process that
        // dies mid-write leaves the old state rather than half of the new one.
        // Rename within a directory is atomic; place_state_file creates the
        // directory, which is the other half of a first run working at all.
        let tmp = match dirs.place_state_file(format!("{}.new", STATE_FILE)) {
            Ok(tmp) => tmp,
            Err(e) => {
                eprintln!("Warning: failed to create state directory: {}", e);
                return;
            }
        };
        let final_path = tmp.with_file_name(STATE_FILE);
        if let Err(e) = std::fs::write(&tmp, text) {
            eprintln!("Warning: failed to write {}: {}", tmp.display(), e);
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &final_path) {
            eprintln!("Warning: failed to save {}: {}", final_path.display(), e);
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// The selection as it stands, in the form `save` would write.
    pub fn of(tool: ToolType, size: Size, color: Color) -> Self {
        Self {
            tool: Some(tool),
            size: Some(size),
            color: Some(color.to_hex()),
        }
    }
}
