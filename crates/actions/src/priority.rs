use crate::{ActionError, open_verified};
use serde::{Deserialize, Serialize};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_CREATION_FLAGS, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, REALTIME_PRIORITY_CLASS, SetPriorityClass,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl PriorityClass {
    pub fn to_windows(self) -> PROCESS_CREATION_FLAGS {
        match self {
            Self::Idle => IDLE_PRIORITY_CLASS,
            Self::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Self::Normal => NORMAL_PRIORITY_CLASS,
            Self::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Self::High => HIGH_PRIORITY_CLASS,
            Self::Realtime => REALTIME_PRIORITY_CLASS,
        }
    }

    pub fn from_windows(flags: u32) -> Option<Self> {
        match flags {
            c if c == IDLE_PRIORITY_CLASS.0 => Some(Self::Idle),
            c if c == BELOW_NORMAL_PRIORITY_CLASS.0 => Some(Self::BelowNormal),
            c if c == NORMAL_PRIORITY_CLASS.0 => Some(Self::Normal),
            c if c == ABOVE_NORMAL_PRIORITY_CLASS.0 => Some(Self::AboveNormal),
            c if c == HIGH_PRIORITY_CLASS.0 => Some(Self::High),
            c if c == REALTIME_PRIORITY_CLASS.0 => Some(Self::Realtime),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Idle => "Low",
            Self::BelowNormal => "Below normal",
            Self::Normal => "Normal",
            Self::AboveNormal => "Above normal",
            Self::High => "High",
            Self::Realtime => "Realtime",
        }
    }
}

pub fn set_priority_class(pid: u32, create_time: i64, priority: PriorityClass) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_SET_INFORMATION)?;
    // SAFETY: verified handle opened with PROCESS_SET_INFORMATION.
    unsafe { SetPriorityClass(handle.0, priority.to_windows()) }.map_err(|e| ActionError::from_win(&e))
}

pub fn get_priority_class(pid: u32, create_time: i64) -> Result<PriorityClass, ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_QUERY_LIMITED_INFORMATION)?;
    // SAFETY: valid handle with query rights.
    let raw = unsafe { GetPriorityClass(handle.0) };
    if raw == 0 {
        return Err(ActionError::from_win(&windows::core::Error::from_thread()));
    }
    PriorityClass::from_windows(raw).ok_or_else(|| ActionError::Other(format!("unknown priority class: {raw:#x}")))
}
