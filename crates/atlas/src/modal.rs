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

use std::io;

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
    /// The settings form, editing a live draft applied on Save.
    Settings(SettingsDraft),
}

/// What a modal footer button does when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalAction {
    /// Dismiss the modal, discarding any draft (OK / Cancel / the close `x`).
    Close,
    /// Apply and persist the settings draft, then dismiss.
    SaveSettings,
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

    /// The settings overlay, editing `draft`.
    pub fn settings(draft: SettingsDraft) -> Self {
        Self {
            title: "Settings".to_string(),
            closable: true,
            body: ModalBody::Settings(draft),
        }
    }

    /// The settings draft, if this is a settings modal (for mutation from events).
    pub fn settings_draft_mut(&mut self) -> Option<&mut SettingsDraft> {
        match &mut self.body {
            ModalBody::Settings(draft) => Some(draft),
            ModalBody::Message(_) => None,
        }
    }

    /// A copy of the settings draft, if this is a settings modal.
    pub fn settings_draft(&self) -> Option<SettingsDraft> {
        match &self.body {
            ModalBody::Settings(draft) => Some(*draft),
            ModalBody::Message(_) => None,
        }
    }

    /// The default (accented) footer action - what Enter confirms.
    pub fn default_action(&self) -> ModalAction {
        match self.body {
            ModalBody::Settings(_) => ModalAction::SaveSettings,
            ModalBody::Message(_) => ModalAction::Close,
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
    let raw = err.to_string();
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
