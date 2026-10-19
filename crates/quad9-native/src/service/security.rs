use super::*;
use std::{
    mem,
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        Com::CoTaskMemFree, Pipes::ImpersonateNamedPipeClient, Threading::*,
        WindowsProgramming::GetComputerNameW,
    },
    UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath},
};

pub(super) fn sid(kind: WELL_KNOWN_SID_TYPE) -> Result<Vec<u32>, NativeError> {
    let mut buffer = vec![0u32; 17];
    let mut bytes = 68;
    // SAFETY: The DWORD-aligned buffer has SECURITY_MAX_SID_SIZE bytes.
    if unsafe {
        CreateWellKnownSid(
            kind,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    } == 0
    {
        return Err(last_error("CreateWellKnownSid"));
    }
    Ok(buffer)
}

fn sid_equal(a: PSID, b: &[u32]) -> bool {
    // SAFETY: Both SIDs came from validated Windows security objects.
    unsafe { EqualSid(a, b.as_ptr().cast_mut().cast()) != 0 }
}

pub(super) fn sid_text(value: PSID) -> Result<String, NativeError> {
    let mut string = ptr::null_mut();
    // SAFETY: value is a live Windows SID and string is a valid output pointer.
    if unsafe { ConvertSidToStringSidW(value, &mut string) } == 0 {
        return Err(last_error("ConvertSidToStringSidW"));
    }
    let allocation = LocalAllocation(string.cast());
    let mut len = 0;
    // SAFETY: Windows allocates a terminated SID string, bounded by SID limits.
    while unsafe { *string.add(len) } != 0 {
        len += 1;
        if len > 184 {
            return Err(invalid("Overlong SID"));
        }
    }
    // SAFETY: The scan established the live initialized string length.
    let result = String::from_utf16(unsafe { std::slice::from_raw_parts(string, len) })
        .map_err(|_| invalid("Invalid SID string"));
    drop(allocation);
    result
}

pub(super) struct Descriptor(pub LocalAllocation);
impl Descriptor {
    pub fn from_sddl(sddl: &str) -> Result<Self, NativeError> {
        let mut descriptor = ptr::null_mut();
        // SAFETY: The fixed/generated SDDL is terminated and the output is valid.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
            ));
        }
        Ok(Self(LocalAllocation(descriptor)))
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0 .0,
            bInheritHandle: 0,
        }
    }
}

pub struct MachineDirectory {
    path: PathBuf,
    _directory: Handle,
}

impl MachineDirectory {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn open() -> Result<Self, NativeError> {
        let mut program_data = ptr::null_mut();
        // SAFETY: The known-folder identifier and out-pointer are valid.
        let hr = unsafe {
            SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, null_handle(), &mut program_data)
        };
        if hr < 0 {
            return Err(invalid("Cannot locate protected machine storage"));
        }
        let mut len = 0;
        // SAFETY: SHGetKnownFolderPath returns a NUL-terminated allocation.
        while unsafe { *program_data.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: The entire string remains owned until CoTaskMemFree below.
        let parent = String::from_utf16(unsafe { std::slice::from_raw_parts(program_data, len) });
        // SAFETY: This is the matching release for SHGetKnownFolderPath.
        unsafe {
            CoTaskMemFree(program_data.cast());
        }
        let path = PathBuf::from(parent.map_err(|_| invalid("Invalid machine storage path"))?)
            .join("Quad9Connect");
        let descriptor = Descriptor::from_sddl("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")?;
        use std::os::windows::ffi::OsStrExt;
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: Directory creation uses a protected DACL from its first instant.
        if unsafe { CreateDirectoryW(name.as_ptr(), &descriptor.attributes()) } == 0 {
            // SAFETY: GetLastError retrieves this thread's immediately preceding error.
            if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
                return Err(last_error("CreateDirectoryW"));
            }
        }
        // Do not share deletion: retain this handle while the service is running.
        // SAFETY: Fixed absolute path and initialized parameters; no final reparse traversal.
        let directory = Handle::checked(
            unsafe {
                CreateFileW(
                    name.as_ptr(),
                    READ_CONTROL | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                    null_handle(),
                )
            },
            "Open recovery directory",
        )?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: info is a correctly sized writable output structure.
        if unsafe { GetFileInformationByHandle(directory.0, &mut info) } == 0 {
            return Err(last_error("GetFileInformationByHandle"));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(invalid("Recovery directory must not be a reparse point"));
        }
        validate_acl(directory.0, true)?;
        Ok(Self {
            path,
            _directory: directory,
        })
    }
}

pub fn validate_private_file(file: &std::fs::File) -> Result<(), NativeError> {
    use std::os::windows::io::AsRawHandle;
    validate_acl(file.as_raw_handle(), false)
}

fn validate_acl(directory: HANDLE, require_protected: bool) -> Result<(), NativeError> {
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: All outputs are valid and remain live in the returned descriptor.
    let code = unsafe {
        GetSecurityInfo(
            directory,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(NativeError::Windows {
            operation: "GetSecurityInfo",
            code,
        });
    }
    let _allocation = LocalAllocation(descriptor);
    let system = sid(WinLocalSystemSid)?;
    let admins = sid(WinBuiltinAdministratorsSid)?;
    if owner.is_null()
        || !(sid_equal(owner, &system) || sid_equal(owner, &admins))
        || dacl.is_null()
    {
        return Err(invalid(
            "Recovery directory has an untrusted owner or missing DACL",
        ));
    }
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor is live and both outputs are writable.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
        return Err(last_error("GetSecurityDescriptorControl"));
    }
    if require_protected && control & SE_DACL_PROTECTED == 0 {
        return Err(invalid("Recovery directory must have a protected DACL"));
    }
    // SAFETY: dacl came from GetSecurityInfo and is valid for this allocation.
    let count = unsafe { (*dacl).AceCount };
    let mut system_full = false;
    let mut admins_full = false;
    for index in 0..count as u32 {
        let mut ace = ptr::null_mut();
        // SAFETY: index is bounded by the ACL's ACE count.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return Err(last_error("GetAce"));
        }
        // SAFETY: The ACE header is present in each valid ACL entry.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if header.AceType != 0 {
            return Err(invalid("Unexpected recovery directory ACE type"));
        }
        // SAFETY: An ACCESS_ALLOWED_ACE starts with this header; SidStart begins its SID.
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let principal = (&allowed.SidStart as *const u32).cast_mut().cast();
        let full = allowed.Mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS;
        if sid_equal(principal, &system) {
            system_full |= full;
        } else if sid_equal(principal, &admins) {
            admins_full |= full;
        } else {
            return Err(invalid(
                "Recovery directory grants access to an untrusted principal",
            ));
        }
    }
    if !system_full || !admins_full {
        return Err(invalid(
            "Recovery directory lacks required administrator and SYSTEM access",
        ));
    }
    Ok(())
}

pub(super) struct PipeSecurity {
    pub descriptor: Descriptor,
    operators: Option<Vec<u32>>,
}

impl PipeSecurity {
    pub fn new() -> Result<Self, NativeError> {
        let mut computer = vec![0u16; 256];
        let mut size = computer.len() as u32;
        // SAFETY: computer is writable for size UTF-16 elements.
        if unsafe { GetComputerNameW(computer.as_mut_ptr(), &mut size) } == 0 {
            return Err(last_error("GetComputerNameW"));
        }
        let name = format!(
            "{}\\{OPERATORS_GROUP}",
            String::from_utf16(&computer[..size as usize])
                .map_err(|_| invalid("Invalid computer name"))?
        );
        let mut buffer = vec![0u32; 17];
        let mut bytes = 68;
        let mut domain = vec![0u16; 256];
        let mut domain_len = 256;
        let mut usage = 0;
        // SAFETY: Buffers have the documented maximum SID size and bounded domain space.
        let found = unsafe {
            LookupAccountNameW(
                ptr::null(),
                wide(&name).as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
                domain.as_mut_ptr(),
                &mut domain_len,
                &mut usage,
            )
        };
        let operators = if found != 0 {
            if usage != SidTypeAlias {
                return Err(invalid("Quad9 operators must be a local group"));
            }
            Some(buffer)
        } else {
            // SAFETY: Read the immediately preceding lookup error.
            if unsafe { GetLastError() } != ERROR_NONE_MAPPED {
                return Err(last_error("LookupAccountNameW"));
            }
            None
        };
        let mut sddl = "D:P(A;;GA;;;SY)(A;;GA;;;BA)".to_owned();
        if let Some(group) = &operators {
            // Data read/write, read-control, synchronize. Exclude CREATE_PIPE_INSTANCE.
            sddl.push_str(&format!(
                "(A;;0x00120003;;;{})",
                sid_text(group.as_ptr().cast_mut().cast())?
            ));
        }
        Ok(Self {
            descriptor: Descriptor::from_sddl(&sddl)?,
            operators,
        })
    }

    pub fn authorize(&self, pipe: HANDLE) -> Result<String, NativeError> {
        // SAFETY: A complete bounded request was read from this connected pipe.
        if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
            return Err(last_error("ImpersonateNamedPipeClient"));
        }
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: This thread entered impersonation above. Failure cannot
                // permit subsequent privileged work under an unknown identity.
                if unsafe { RevertToSelf() } == 0 {
                    std::process::abort();
                }
            }
        }
        let _revert = Revert;
        let mut token = null_handle();
        // SAFETY: Query only the current thread's impersonation token.
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            return Err(last_error("OpenThreadToken"));
        }
        let token = Handle::checked(token, "OpenThreadToken")?;
        let admins = sid(WinBuiltinAdministratorsSid)?;
        let mut allowed = membership(token.0, &admins)?;
        if let Some(operators) = &self.operators {
            allowed |= membership(token.0, operators)?;
        }
        let actor = token_sid(token.0)?;
        allowed |= actor == "S-1-5-18";
        if !allowed {
            return Err(invalid("Caller is not an authorized Quad9 operator"));
        }
        Ok(actor)
    }
}

fn membership(token: HANDLE, group: &[u32]) -> Result<bool, NativeError> {
    let mut member = 0;
    // SAFETY: token is a live query handle and group is a validated SID.
    if unsafe { CheckTokenMembership(token, group.as_ptr().cast_mut().cast(), &mut member) } == 0 {
        return Err(last_error("CheckTokenMembership"));
    }
    Ok(member != 0)
}

pub(super) fn token_sid(token: HANDLE) -> Result<String, NativeError> {
    let mut size = 0;
    // SAFETY: This first call only requests the size of TOKEN_USER.
    unsafe {
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
    }
    if size == 0 || size > 4096 {
        return Err(invalid("Invalid token identity size"));
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: The aligned buffer has the size requested by Windows.
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(last_error("GetTokenInformation"));
    }
    // SAFETY: A successful TokenUser query initialized this structure and SID.
    sid_text(unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}
