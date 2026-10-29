//! Throttle key maps: layout-dependent rate control keys.

/// How much one key press changes a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepSize {
    /// 1 KiB/s.
    Small,
    /// 5 KiB/s.
    Medium,
    /// 50 KiB/s.
    Large,
}

impl StepSize {
    /// Step in bytes per second.
    pub fn bytes(self) -> u64 {
        match self {
            Self::Small => 1024,
            Self::Medium => 5 * 1024,
            Self::Large => 50 * 1024,
        }
    }
}

/// One throttle key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleAction {
    /// Raise the upload limit.
    UploadUp(StepSize),
    /// Lower the upload limit.
    UploadDown(StepSize),
    /// Raise the download limit.
    DownloadUp(StepSize),
    /// Lower the download limit.
    DownloadDown(StepSize),
}

impl ThrottleAction {
    /// rTorrent command that applies this action.
    pub fn command(self) -> &'static str {
        match self {
            Self::UploadUp(_) | Self::UploadDown(_) => "throttle.global_up.max_rate.set",
            Self::DownloadUp(_) | Self::DownloadDown(_) => "throttle.global_down.max_rate.set",
        }
    }

    /// Step carried by this action.
    pub fn step(self) -> StepSize {
        match self {
            Self::UploadUp(step)
            | Self::UploadDown(step)
            | Self::DownloadUp(step)
            | Self::DownloadDown(step) => step,
        }
    }

    /// `true` when the action raises the limit.
    pub fn is_increase(self) -> bool {
        matches!(self, Self::UploadUp(_) | Self::DownloadUp(_))
    }
}

/// Maps layout-specific letter keys onto throttle actions.
///
/// Lowercase keys change the upload limit, uppercase the download limit, in both
/// directions (the classic rTorrent binding set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrottleKeyMap {
    upload_up: [char; 3],
    upload_down: [char; 3],
}

impl ThrottleKeyMap {
    /// Builds the map for `layout`; unknown names fall back to `qwerty`.
    pub fn new(layout: &str) -> Self {
        match layout {
            "azerty" => Self {
                upload_up: ['q', 's', 'd'],
                upload_down: ['w', 'x', 'c'],
            },
            "qwertz" => Self {
                upload_up: ['a', 's', 'd'],
                upload_down: ['y', 'x', 'c'],
            },
            "dvorak" => Self {
                upload_up: ['a', 'o', 'e'],
                upload_down: [';', 'q', 'j'],
            },
            _ => Self {
                upload_up: ['a', 's', 'd'],
                upload_down: ['z', 'x', 'c'],
            },
        }
    }

    /// Action bound to `key`, if any.
    pub fn lookup(&self, key: char) -> Option<ThrottleAction> {
        let download = key.is_ascii_uppercase();
        let wanted = key.to_ascii_lowercase();

        if let Some(index) = self
            .upload_up
            .iter()
            .position(|candidate| *candidate == wanted)
        {
            let step = step_at(index);
            return Some(if download {
                ThrottleAction::DownloadUp(step)
            } else {
                ThrottleAction::UploadUp(step)
            });
        }

        if let Some(index) = self
            .upload_down
            .iter()
            .position(|candidate| *candidate == wanted)
        {
            let step = step_at(index);
            return Some(if download {
                ThrottleAction::DownloadDown(step)
            } else {
                ThrottleAction::UploadDown(step)
            });
        }

        None
    }

    /// Applies `action` to `limit`, saturating at zero and at `u64::MAX`.
    pub fn adjust(limit: u64, action: ThrottleAction) -> u64 {
        if action.is_increase() {
            limit.saturating_add(action.step().bytes())
        } else {
            limit.saturating_sub(action.step().bytes())
        }
    }
}

/// Step bound to a column of the map.
fn step_at(index: usize) -> StepSize {
    match index {
        0 => StepSize::Small,
        1 => StepSize::Medium,
        _ => StepSize::Large,
    }
}
