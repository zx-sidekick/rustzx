use core::fmt;

/// Implements `Display`, with each variant's message, and `core::error::Error`
macro_rules! messages {
    ($name:ident { $($variant:ident => $message:literal),* $(,)? }) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self {
                    $(Self::$variant => $message,)*
                })
            }
        }

        impl core::error::Error for $name {}
    };
}

#[derive(Debug)]
pub enum Error {
    /// Failed to read asset
    AssetRead(IoError),
    /// Failed to load rom
    RomLoad(RomLoadError),
    /// Failed to load tape
    TapeLoad(TapeLoadError),
    /// Failed to load screen
    ScreenLoad(ScreenLoadError),
    /// Failed to load snapshot
    SnapshotLoad(SnapshotLoadError),
    /// Failed to save snapshot
    SnapshotSave(SnapshotSaveError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AssetRead(e) => write!(f, "Failed to read asset: {e}"),
            Self::RomLoad(e) => write!(f, "Failed to load rom: {e}"),
            Self::TapeLoad(e) => write!(f, "Failed to load tape: {e}"),
            Self::ScreenLoad(e) => write!(f, "Failed to load screen: {e}"),
            Self::SnapshotLoad(e) => write!(f, "Failed to load snapshot: {e}"),
            Self::SnapshotSave(e) => write!(f, "Failed to save snapshot: {e}"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(match self {
            Self::AssetRead(e) => e,
            Self::RomLoad(e) => e,
            Self::TapeLoad(e) => e,
            Self::ScreenLoad(e) => e,
            Self::SnapshotLoad(e) => e,
            Self::SnapshotSave(e) => e,
        })
    }
}

macro_rules! from_variants {
    ($($variant:ident($inner:ident)),* $(,)?) => {
        $(impl From<$inner> for Error {
            fn from(e: $inner) -> Self {
                Self::$variant(e)
            }
        })*
    };
}

from_variants!(
    AssetRead(IoError),
    RomLoad(RomLoadError),
    TapeLoad(TapeLoadError),
    ScreenLoad(ScreenLoadError),
    SnapshotLoad(SnapshotLoadError),
    SnapshotSave(SnapshotSaveError),
);

#[derive(Debug)]
pub enum IoError {
    /// Unexpected end of file
    UnexpectedEof,
    /// Sink unexpectedly refused to write more bytes
    WriteZero,
    /// Seek operation was performed with offset before beginning of the asset
    SeekBeforeStart,
    /// Host-provided asset implementation failed
    HostAssetImplFailed,
}

messages!(IoError {
    UnexpectedEof => "Unexpected end of file",
    WriteZero => "Sink unexpectedly refused to write more bytes",
    SeekBeforeStart => "Seek operation was performed with offset before beginning of the asset",
    HostAssetImplFailed => "Host-provided asset implementation failed",
});

#[derive(Debug)]
pub enum RomLoadError {
    /// More assets required to load rom
    MoreAssetsRequired,
}

messages!(RomLoadError {
    MoreAssetsRequired => "More assets required to load rom",
});

#[derive(Debug)]
pub enum TapeLoadError {
    /// Provided tap file is invalid
    InvalidTapFile,
}

messages!(TapeLoadError {
    InvalidTapFile => "Provided tap file is invalid",
});

#[derive(Debug)]
pub enum SnapshotLoadError {
    /// Provided SNA file is invalid
    InvalidSNAFile,
    /// Provided SZX file is invalid
    InvalidSZXFile,
    /// Machine required by snapshot isn't supported
    MachineNotSupported,
    /// Zlib not supported
    ZlibNotSupported,
}

messages!(SnapshotLoadError {
    InvalidSNAFile => "Provided SNA file is invalid",
    InvalidSZXFile => "Provided SZX file is invalid",
    MachineNotSupported => "Machine required by snapshot isn't supported",
    ZlibNotSupported => "Zlib not supported",
});

#[derive(Debug)]
pub enum SnapshotSaveError {
    /// Save not supported
    NotSupported,
}

messages!(SnapshotSaveError {
    NotSupported => "Save not supported",
});

#[derive(Debug)]
pub enum ScreenLoadError {
    /// Provided scr file is invalid
    InvalidScrFile,
    /// Selected machine can't be used to load given screen file
    MachineNotSupported,
}

messages!(ScreenLoadError {
    InvalidScrFile => "Provided scr file is invalid",
    MachineNotSupported => "Selected machine can't be used to load given screen file",
});
