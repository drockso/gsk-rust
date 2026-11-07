//! GSK file-transfer protocol, implemented directly over UDP. No vendor DLL/FFI.
#![forbid(unsafe_code)]

pub mod client;
pub mod wire;

use std::{fmt, io};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Invalid(String),
    Protocol(String),
    Remote { operation: u8, code: u16 },
    Timeout { operation: u8, uncertain: bool },
    Verification(String),
    Cancelled,
}
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Invalid(s) => write!(f, "{s}"),
            Self::Protocol(s) => write!(f, "Invalid GSK response: {s}"),
            Self::Remote { operation, code } => {
                let meaning = match code {
                    0x8011 => "end of file",
                    0x8012 => "file session is not mounted",
                    0x8016 => "file or path not found",
                    _ => "controller rejected the operation",
                };
                write!(
                    f,
                    "GSK 0x{code:04X}, operation 0x{operation:02X}: {meaning}"
                )
            }
            Self::Timeout {
                operation,
                uncertain,
            } => {
                write!(f, "Timeout waiting for GSK operation 0x{operation:02X}")?;
                if *uncertain {
                    write!(
                        f,
                        "; the operation may have completed on the controller; it was not automatically replayed"
                    )?;
                }
                Ok(())
            }
            Self::Verification(s) => write!(f, "Verification failed: {s}"),
            Self::Cancelled => write!(
                f,
                "Cancelled; an interrupted upload may leave a partial remote file"
            ),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl Error {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Invalid(_) => 2,
            Self::Verification(_) => 5,
            Self::Cancelled => 130,
            _ => 4,
        }
    }
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Remote { code: 0x8016, .. })
    }
}
