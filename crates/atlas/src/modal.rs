//! atlas modal dialogs: a blocking, centered dialog over a dimming backdrop.
//!
//! This is the data model only - `ui.rs` renders it (`build_modal`) and `main.rs`
//! drives it (open/close, key/press interception, button dispatch). A modal
//! "takes over context": while one is open the backdrop popup swallows every
//! click (see `Ui::hit_test`, which tests popups first) and the editor's own
//! keyboard shortcuts are suppressed.
//!
//! Two bodies ship: a [`ModalBody::Message`] (errors/info) and a
//! [`ModalBody::Settings`] form editing a [`SettingsDraft`].

use std::fmt;
use std::io;

use helios::NodeId;

use crate::explorer::AssetRef;
use crate::settings::SnapSettings;

/// An open modal dialog: a title, whether it can be dismissed without choosing a
/// button (close `x` / Escape / backdrop click), and its content.
pub struct Modal {
    pub title: String,
    pub closable: bool,
    pub body: ModalBody,
}

/// A modal's content.
pub enum ModalBody {
    /// A block of message text (an error or an informational notice).
    Message(String),
    /// A question with three answers, asked before something would drop unsaved
    /// work: save it, throw it away, or do not do the thing at all.
    Confirm(Confirm),
    /// The settings form, editing a live draft applied on Save.
    Settings(SettingsDraft),
    /// An image picker: a grid of the project's images; clicking one sets the
    /// target field.
    AssetChooser(AssetChooser),
}

/// A "you have unsaved changes" question, and what to do once it is answered.
pub struct Confirm {
    pub message: String,
    /// What the caller wanted to do, held until the answer comes back.
    pub pending: Pending,
}

/// The action a [`Confirm`] is guarding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Close the window.
    Quit,
    /// Open a different script into the Code pane.
    OpenScript(std::path::PathBuf),
}

/// Which reflected component field an asset chooser writes to.
#[derive(Debug, Clone, Copy)]
pub struct AssetTarget {
    pub node: NodeId,
    pub component: usize,
    pub field: &'static str,
}

/// The state behind the asset chooser: the field being set and the assets to
/// choose among.
pub struct AssetChooser {
    pub target: AssetTarget,
    pub assets: Vec<AssetRef>,
    pub kind: AssetKind,
}

/// Which kind of asset a chooser is offering. An asset field is not generic in
/// practice - a sprite wants an image and a script wants a `.cmt` - so the
/// chooser lists only what the field can actually hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Image,
    Script,
}

impl AssetKind {
    /// The chooser's title.
    pub fn title(self) -> &'static str {
        match self {
            AssetKind::Image => "Choose an image",
            AssetKind::Script => "Choose a script",
        }
    }

    /// What to say when the project has none of them.
    pub fn empty_message(self) -> &'static str {
        match self {
            AssetKind::Image => "No images in this project.",
            AssetKind::Script => "No scripts in this project.",
        }
    }
}

/// What a modal footer button does when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalAction {
    /// Dismiss the modal, discarding any draft (OK / Cancel / the close `x`).
    Close,
    /// Apply and persist the settings draft, then dismiss.
    SaveSettings,
    /// Save the open script, then do what the confirm was guarding.
    SaveThenProceed,
    /// Throw the unsaved edits away and do it anyway.
    DiscardAndProceed,
}

/// A field of the settings form, so a clicked checkbox / submitted input maps
/// back to the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingField {
    ShowGrid,
    ShowAxes,
    SnapEnabled,
    MoveStep,
    RotateStep,
    ScaleStep,
}

impl Modal {
    /// An error dialog for a file-operation failure, with a user-friendly
    /// message derived from the error.
    pub fn error(title: impl Into<String>, err: &io::Error) -> Self {
        Self {
            title: title.into(),
            closable: true,
            body: ModalBody::Message(friendly_error(err)),
        }
    }

    /// An error dialog for any displayable failure (e.g. a save/load error that
    /// is not a bare `io::Error`), its message read back as a sentence. Prefer
    /// [`error`](Self::error) for file-op io errors, which get friendlier,
    /// kind-specific wording (e.g. a name collision).
    pub fn report(title: impl Into<String>, err: impl fmt::Display) -> Self {
        Self {
            title: title.into(),
            closable: true,
            body: ModalBody::Message(humanize(&err.to_string())),
        }
    }

    /// Ask before dropping unsaved work. Not closable by Escape alone: the
    /// three answers are the only ways out, because dismissing it would have to
    /// mean one of them and there is no safe guess.
    pub fn confirm(message: impl Into<String>, pending: Pending) -> Self {
        Self {
            title: "Unsaved changes".to_string(),
            closable: true,
            body: ModalBody::Confirm(Confirm {
                message: message.into(),
                pending,
            }),
        }
    }

    /// The settings overlay, editing `draft`.
    pub fn settings(draft: SettingsDraft) -> Self {
        Self {
            title: "Settings".to_string(),
            closable: true,
            body: ModalBody::Settings(draft),
        }
    }

    /// The asset chooser for an image field.
    pub fn asset_chooser(target: AssetTarget, assets: Vec<AssetRef>, kind: AssetKind) -> Self {
        Self {
            title: kind.title().to_string(),
            closable: true,
            body: ModalBody::AssetChooser(AssetChooser {
                target,
                assets,
                kind,
            }),
        }
    }

    /// The settings draft, if this is a settings modal (for mutation from events).
    pub fn settings_draft_mut(&mut self) -> Option<&mut SettingsDraft> {
        match &mut self.body {
            ModalBody::Settings(draft) => Some(draft),
            _ => None,
        }
    }

    /// A copy of the settings draft, if this is a settings modal.
    pub fn settings_draft(&self) -> Option<SettingsDraft> {
        match &self.body {
            ModalBody::Settings(draft) => Some(*draft),
            _ => None,
        }
    }

    /// The asset chooser's target field, if this is an asset chooser.
    pub fn asset_target(&self) -> Option<AssetTarget> {
        match &self.body {
            ModalBody::AssetChooser(c) => Some(c.target),
            _ => None,
        }
    }

    /// The asset chooser's images, if this is an asset chooser (to ensure their
    /// thumbnails before rendering the grid).
    pub fn asset_images(&self) -> Option<&[AssetRef]> {
        match &self.body {
            ModalBody::AssetChooser(c) => Some(&c.assets),
            _ => None,
        }
    }

    /// The default (accented) footer action - what Enter confirms.
    pub fn default_action(&self) -> ModalAction {
        match self.body {
            ModalBody::Settings(_) => ModalAction::SaveSettings,
            // Enter on an unsaved-changes question saves. Of the three answers
            // it is the only one that cannot lose work, which is what a default
            // has to be.
            ModalBody::Confirm(_) => ModalAction::SaveThenProceed,
            ModalBody::Message(_) | ModalBody::AssetChooser(_) => ModalAction::Close,
        }
    }
}

/// The editable values behind the settings form. Booleans drive checkboxes;
/// the numbers drive numeric inputs (formatted from / parsed back into these).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingsDraft {
    pub show_grid: bool,
    pub show_axes: bool,
    pub snap_enabled: bool,
    pub move_step: f32,
    pub rotate_step_deg: f32,
    pub scale_step: f32,
}

impl SettingsDraft {
    /// Seed a draft from the current view toggles and snap settings.
    pub fn new(show_grid: bool, show_axes: bool, snap: SnapSettings) -> Self {
        Self {
            show_grid,
            show_axes,
            snap_enabled: snap.enabled,
            move_step: snap.move_step,
            rotate_step_deg: snap.rotate_step_deg,
            scale_step: snap.scale_step,
        }
    }

    /// The snapping settings this draft describes (to write back on Save).
    pub fn snap(&self) -> SnapSettings {
        SnapSettings {
            enabled: self.snap_enabled,
            move_step: self.move_step,
            rotate_step_deg: self.rotate_step_deg,
            scale_step: self.scale_step,
        }
    }

    /// A boolean field's current value (for rendering a checkbox).
    pub fn checked(&self, field: SettingField) -> bool {
        match field {
            SettingField::ShowGrid => self.show_grid,
            SettingField::ShowAxes => self.show_axes,
            SettingField::SnapEnabled => self.snap_enabled,
            _ => false,
        }
    }

    /// Set a boolean field (from a checkbox toggle).
    pub fn set_checked(&mut self, field: SettingField, on: bool) {
        match field {
            SettingField::ShowGrid => self.show_grid = on,
            SettingField::ShowAxes => self.show_axes = on,
            SettingField::SnapEnabled => self.snap_enabled = on,
            _ => {}
        }
    }

    /// A numeric field's current value (for formatting an input), or `None` for a
    /// boolean field.
    pub fn number(&self, field: SettingField) -> Option<f32> {
        match field {
            SettingField::MoveStep => Some(self.move_step),
            SettingField::RotateStep => Some(self.rotate_step_deg),
            SettingField::ScaleStep => Some(self.scale_step),
            _ => None,
        }
    }

    /// Set a numeric field (from a submitted input). Non-finite or non-positive
    /// values are ignored, so a bad edit cannot break snapping.
    pub fn set_number(&mut self, field: SettingField, value: f32) {
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        match field {
            SettingField::MoveStep => self.move_step = value,
            SettingField::RotateStep => self.rotate_step_deg = value,
            SettingField::ScaleStep => self.scale_step = value,
            _ => {}
        }
    }
}

/// A short, user-facing message for a file-operation error. The
/// collision case (the one the user hits by renaming onto an existing name)
/// gets a fixed sentence; anything else uses the error's own text, capitalized
/// and period-terminated.
fn friendly_error(err: &io::Error) -> String {
    if err.kind() == io::ErrorKind::AlreadyExists {
        return "A file with that name already exists.".to_string();
    }
    humanize(&err.to_string())
}

/// Read a raw error string back as a sentence: capitalize the first letter and
/// add terminating punctuation if it has none. An empty string becomes a
/// generic fallback.
fn humanize(raw: &str) -> String {
    let mut chars = raw.chars();
    let mut msg = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => return "The operation failed.".to_string(),
    };
    if !msg.ends_with(['.', '!', '?']) {
        msg.push('.');
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_message_is_friendly_for_a_collision_and_capitalized_otherwise() {
        let exists = io::Error::new(io::ErrorKind::AlreadyExists, "a file with that name...");
        let modal = Modal::error("Rename failed", &exists);
        assert_eq!(modal.title, "Rename failed");
        assert!(modal.closable);
        match modal.body {
            ModalBody::Message(m) => assert_eq!(m, "A file with that name already exists."),
            _ => panic!("expected a message body"),
        }

        let other = io::Error::new(io::ErrorKind::InvalidInput, "invalid name");
        match Modal::error("x", &other).body {
            ModalBody::Message(m) => assert_eq!(m, "Invalid name."),
            _ => panic!("expected a message body"),
        }
    }

    #[test]
    fn report_reads_any_display_error_as_a_sentence() {
        // A save/load HeliosError is not an io::Error; report() humanizes its
        // Display text (capitalized, period-terminated) the same way.
        let modal = Modal::report("Save failed", "parsing scene RON: expected ']'");
        assert_eq!(modal.title, "Save failed");
        assert!(modal.closable);
        match modal.body {
            ModalBody::Message(m) => assert_eq!(m, "Parsing scene RON: expected ']'."),
            _ => panic!("expected a message body"),
        }
    }

    #[test]
    fn settings_draft_round_trips_and_guards_bad_numbers() {
        let snap = SnapSettings {
            enabled: true,
            move_step: 16.0,
            rotate_step_deg: 15.0,
            scale_step: 0.1,
        };
        let mut draft = SettingsDraft::new(true, false, snap);
        assert!(draft.checked(SettingField::ShowGrid));
        assert!(!draft.checked(SettingField::ShowAxes));
        assert_eq!(draft.number(SettingField::MoveStep), Some(16.0));
        assert_eq!(draft.number(SettingField::ShowGrid), None);
        // Round-trips the snapping settings.
        assert_eq!(draft.snap(), snap);

        // Edits apply; a non-positive / non-finite number is rejected.
        draft.set_checked(SettingField::ShowAxes, true);
        draft.set_number(SettingField::MoveStep, 32.0);
        draft.set_number(SettingField::MoveStep, -1.0);
        draft.set_number(SettingField::MoveStep, f32::NAN);
        assert!(draft.show_axes);
        assert_eq!(draft.move_step, 32.0);
    }
}
