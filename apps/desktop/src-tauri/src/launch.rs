// SPDX-License-Identifier: AGPL-3.0-or-later
use std::ffi::OsStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Window,
    Tray,
}

impl LaunchMode {
    // Excludes argv[0]. Startup arguments never select a service operation.
    pub fn parse<I, S>(arguments: I) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut arguments = arguments.into_iter();
        match (arguments.next(), arguments.next()) {
            (None, None) => Ok(Self::Window),
            (Some(argument), None) if argument.as_ref() == "--start-in-tray" => Ok(Self::Tray),
            _ => Err("Usage: quad9-desktop.exe [--start-in-tray]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_tray_start_is_separate_from_normal_window_activation() {
        assert_eq!(LaunchMode::parse([] as [&str; 0]), Ok(LaunchMode::Window));
        assert_eq!(LaunchMode::parse(["--start-in-tray"]), Ok(LaunchMode::Tray));
    }

    #[test]
    fn reject_unsupported_or_ambiguous_launch_requests() {
        for arguments in [
            vec!["--enable"],
            vec!["--restore"],
            vec!["--start-in-tray=true"],
            vec!["--START-IN-TRAY"],
            vec!["--start-in-tray", "--start-in-tray"],
            vec!["--start-in-tray", "--enable"],
            vec!["quad9://enable"],
            vec![""],
        ] {
            assert!(LaunchMode::parse(arguments).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_arguments_are_rejected_without_lossy_matching() {
        use std::os::windows::ffi::OsStringExt;
        let argument = std::ffi::OsString::from_wide(&[0xd800]);
        assert!(LaunchMode::parse([argument]).is_err());
    }
}
