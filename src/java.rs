use anyhow::{bail, Context};
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};
use tempfile::TempDir;

#[cfg(target_os = "windows")]
const JAVA_EXE_NAME: &str = "javaw.exe";
#[cfg(not(target_os = "windows"))]
const JAVA_EXE_NAME: &str = "java";

#[cfg(target_os = "windows")]
#[allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]
mod reg {
    use std::borrow::Cow;
    use std::ffi::c_void;
    use std::mem::MaybeUninit;
    use std::path::PathBuf;
    use std::{io, ptr};

    type HKEY = *mut c_void;
    type LPCWSTR = *const u16;
    type LPWSTR = *mut u16;
    type REGSAM = u32;
    type LSTATUS = u32;

    const ERROR_SUCCESS: LSTATUS = 0;
    const HKEY_LOCAL_MACHINE: HKEY = -2147483646i32 as _;
    const KEY_ENUMERATE_SUB_KEYS: u32 = 0x0008;
    const KEY_READ: u32 = 0x20019;
    pub(super) const KEY_WOW64_64KEY: u32 = 0x0100;
    pub(super) const KEY_WOW64_32KEY: u32 = 0x0200;

    #[repr(C)]
    struct FILETIME {
        dwLowDateTime: u32,
        dwHighDateTime: u32,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hKey: HKEY,
            lpSubKey: LPCWSTR,
            ulOptions: u32,
            samDesired: REGSAM,
            phkResult: *mut HKEY,
        ) -> LSTATUS;
        fn RegCloseKey(hKey: HKEY) -> LSTATUS;
        fn RegQueryInfoKeyW(
            hKey: HKEY,
            lpClass: LPWSTR,
            lpcchClass: *mut u32,
            lpReserved: *mut u32,
            lpcSubKeys: *mut u32,
            lpcbMaxSubKeyLen: *mut u32,
            lpcbMaxClassLen: *mut u32,
            lpcValues: *mut u32,
            lpcbMaxValueNameLen: *mut u32,
            lpcbMaxValueLen: *mut u32,
            lpcbSecurityDescriptor: *mut u32,
            lpftLastWriteTime: *mut FILETIME,
        ) -> LSTATUS;
        fn RegEnumKeyExW(
            hKey: HKEY,
            dwIndex: u32,
            lpName: LPWSTR,
            lpcchName: *mut u32,
            lpReserved: *mut u32,
            lpClass: LPWSTR,
            lpcchClass: *mut u32,
            lpftLastWriteTime: *mut FILETIME,
        ) -> LSTATUS;
        fn RegQueryValueExW(
            hKey: HKEY,
            lpValueName: LPCWSTR,
            lpReserved: *mut u32,
            lpType: *mut u32,
            lpByte: *mut u8,
            lpcbData: *mut u32,
        ) -> LSTATUS;
    }

    struct RegistryKey<'a> {
        key: HKEY,
        key_name: Cow<'a, [u16]>,
    }

    impl<'a> RegistryKey<'a> {
        fn open(key_name: impl Into<Cow<'a, [u16]>>, flags: REGSAM) -> Option<RegistryKey<'a>> {
            let key_name = key_name.into();
            assert!(key_name.ends_with(&[0]));

            unsafe {
                let mut key = MaybeUninit::uninit();
                let result = RegOpenKeyExW(
                    HKEY_LOCAL_MACHINE,
                    key_name.as_ptr(),
                    0,
                    flags,
                    key.as_mut_ptr(),
                );
                if result != ERROR_SUCCESS {
                    return None;
                }
                Some(RegistryKey {
                    key: key.assume_init(),
                    key_name,
                })
            }
        }

        fn iter<'b>(
            &'b self,
            sub_key_suffix: &'b [u16],
            flags: REGSAM,
        ) -> io::Result<impl Iterator<Item = RegistryKey<'static>> + 'b> {
            let num_sub_keys = unsafe {
                let mut num_sub_keys = MaybeUninit::uninit();
                let err = RegQueryInfoKeyW(
                    self.key,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    num_sub_keys.as_mut_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                if err != ERROR_SUCCESS {
                    return Err(io::Error::from_raw_os_error(err as i32));
                }
                num_sub_keys.assume_init()
            };

            Ok((0..num_sub_keys).filter_map(move |i| {
                let sub_key_name = unsafe {
                    let mut sub_key_name_size = 255;
                    let mut sub_key_name = Vec::with_capacity(sub_key_name_size as usize);
                    if RegEnumKeyExW(
                        self.key,
                        i,
                        sub_key_name.as_mut_ptr(),
                        &mut sub_key_name_size,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                    ) != ERROR_SUCCESS
                    {
                        return None;
                    }
                    sub_key_name.set_len(sub_key_name_size as usize);
                    sub_key_name
                };

                let mut new_key_name = Vec::with_capacity(
                    self.key_name.len() - 1 // key_name
                        + 1 // \
                        + sub_key_name.len() // sub_key_name
                        + sub_key_suffix.len() - 1 // sub_key_suffix
                        + 1, // \0
                );
                new_key_name.extend_from_slice(&self.key_name.as_ref()[..self.key_name.len() - 1]);
                new_key_name.push(b'\\' as u16);
                new_key_name.extend_from_slice(&sub_key_name);
                new_key_name.extend_from_slice(&sub_key_suffix[..sub_key_suffix.len() - 1]);
                new_key_name.push(0);

                RegistryKey::open(new_key_name, flags)
            }))
        }

        fn get(&self, key: &[u16]) -> anyhow::Result<String> {
            assert!(key.ends_with(&[0]));

            unsafe {
                let mut value_size = 0;
                let err = RegQueryValueExW(
                    self.key,
                    key.as_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut value_size,
                );
                if err != ERROR_SUCCESS {
                    return Err(io::Error::from_raw_os_error(err as i32).into());
                }
                let mut value = Vec::<u16>::with_capacity(value_size as usize);
                let err = RegQueryValueExW(
                    self.key,
                    key.as_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    value.as_mut_ptr() as *mut u8,
                    &mut value_size,
                );
                if err != ERROR_SUCCESS {
                    return Err(io::Error::from_raw_os_error(err as i32).into());
                }
                value.set_len(value_size as usize);
                if value.ends_with(&[0]) {
                    value.pop();
                }
                Ok(String::from_utf16(&value)?)
            }
        }
    }

    impl Drop for RegistryKey<'_> {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.key);
            }
        }
    }

    const fn wstr<const N: usize>(str: &[u8; N]) -> [u16; N] {
        assert!(matches!(str.last(), Some(&0)));
        assert!(str.is_ascii());
        let mut result = [0; N];
        let mut i = 0;
        while i < str.len() {
            result[i] = str[i] as u16;
            i += 1;
        }
        result
    }

    pub(super) fn find_java_from_registry_key(
        key_type: u32,
        key_name: &[u16],
        key_java_dir: &[u16],
        sub_key_suffix: &[u16],
    ) -> anyhow::Result<Vec<PathBuf>> {
        let arch_type = match key_type {
            KEY_WOW64_64KEY => "64",
            KEY_WOW64_32KEY => "32",
            _ => "unknown",
        };

        let Some(jre_key) =
            RegistryKey::open(key_name, KEY_READ | key_type | KEY_ENUMERATE_SUB_KEYS)
        else {
            return Ok(Vec::new());
        };
        let result = jre_key
            .iter(sub_key_suffix, KEY_READ | KEY_WOW64_64KEY)?
            .filter_map(|key| {
                key.get(key_java_dir)
                    .ok()
                    .map(|value| [&value, "bin", "javaw.exe"].iter().collect())
            })
            .collect();
        Ok(result)
    }

    pub(super) const ORACLE_J8_JRE_KEY: &[u16] =
        &wstr(b"SOFTWARE\\JavaSoft\\Java Runtime Environment\0");
    pub(super) const ORACLE_J8_JDK_KEY: &[u16] =
        &wstr(b"SOFTWARE\\JavaSoft\\Java Development Kit\0");
    pub(super) const ORACLE_JRE_KEY: &[u16] = &wstr(b"SOFTWARE\\JavaSoft\\JRE\0");
    pub(super) const ORACLE_JDK_KEY: &[u16] = &wstr(b"SOFTWARE\\JavaSoft\\JDK\0");
    pub(super) const ORACLE_KEY_JAVA_DIR: &[u16] = &wstr(b"JavaHome\0");

    pub(super) const ADOPTOPENJDK_JRE_KEY: &[u16] = &wstr(b"SOFTWARE\\AdoptOpenJDK\\JRE\0");
    pub(super) const ADOPTOPENJDK_JDK_KEY: &[u16] = &wstr(b"SOFTWARE\\AdoptOpenJDK\\JDK\0");

    pub(super) const ECLIPSE_JDK_KEY: &[u16] = &wstr(b"SOFTWARE\\Eclipse Foundation\\JDK\0");

    pub(super) const ADOPTIUM_JRE_KEY: &[u16] = &wstr(b"SOFTWARE\\Eclipse Adoptium\\JRE\0");
    pub(super) const ADOPTIUM_JDK_KEY: &[u16] = &wstr(b"SOFTWARE\\Eclipse Adoptium\\JDK\0");
    pub(super) const ADOPTIUM_KEY_JAVA_DIR: &[u16] = &wstr(b"Path\0");
    pub(super) const ADOPTIUM_SUB_KEY_SUFFIX: &[u16] = &wstr(b"\\hotspot\\MSI\0");

    pub(super) const MICROSOFT_JDK_KEY: &[u16] = &wstr(b"SOFTWARE\\Microsoft\\JDK\0");

    pub(super) const ZULU_JDK_KEY: &[u16] = &wstr(b"SOFTWARE\\Azul Systems\\Zulu\0");
    pub(super) const ZULU_KEY_JAVA_DIR: &[u16] = &wstr(b"InstallationPath\0");

    pub(super) const LIBERICA_JDK_KEY: &[u16] = &wstr(b"SOFTWARE\\BellSoft\\Liberica\0");

    pub(super) const EMPTY_STRING: &[u16] = &[0];
}

#[cfg(target_os = "windows")]
fn find_platform_specific_java_candidates() -> anyhow::Result<Vec<PathBuf>> {
    // Oracle
    let jre64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ORACLE_J8_JRE_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let jdk64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ORACLE_J8_JDK_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let jre32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ORACLE_J8_JRE_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let jdk32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ORACLE_J8_JDK_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;

    // Oracle for Java 9 and newer
    let new_jre64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ORACLE_JRE_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let new_jdk64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ORACLE_JDK_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let new_jre32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ORACLE_JRE_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let new_jdk32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ORACLE_JDK_KEY,
        reg::ORACLE_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;

    // AdoptOpenJDK
    let adopt_open_jre32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ADOPTOPENJDK_JRE_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;
    let adopt_open_jre64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ADOPTOPENJDK_JRE_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;
    let adopt_open_jdk32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ADOPTOPENJDK_JDK_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;
    let adopt_open_jdk64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ADOPTOPENJDK_JDK_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;

    // Eclipse Foundation
    let foundation_jdk32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ECLIPSE_JDK_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;
    let foundation_jdk64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ECLIPSE_JDK_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;

    // Eclipse Adoptium
    let adoptium_jre32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ADOPTIUM_JRE_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;
    let adoptium_jre64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ADOPTIUM_JRE_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;
    let adoptium_jdk32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ADOPTIUM_JDK_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;
    let adoptium_jdk64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ADOPTIUM_JDK_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;

    // Microsoft
    let microsoft_jdk64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::MICROSOFT_JDK_KEY,
        reg::ADOPTIUM_KEY_JAVA_DIR,
        reg::ADOPTIUM_SUB_KEY_SUFFIX,
    )?;

    // Azul Zulu
    let zulu_64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::ZULU_JDK_KEY,
        reg::ZULU_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let zulu_32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::ZULU_JDK_KEY,
        reg::ZULU_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;

    // BellSoft Liberica
    let liberica_64s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_64KEY,
        reg::LIBERICA_JDK_KEY,
        reg::ZULU_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;
    let liberica_32s = reg::find_java_from_registry_key(
        reg::KEY_WOW64_32KEY,
        reg::LIBERICA_JDK_KEY,
        reg::ZULU_KEY_JAVA_DIR,
        reg::EMPTY_STRING,
    )?;

    // List x64 before x86
    let mut java_candidates = Vec::new();
    java_candidates.extend(jre64s);
    java_candidates.extend(new_jre64s);
    java_candidates.extend(adopt_open_jre64s);
    java_candidates.extend(adoptium_jre64s);
    java_candidates.push(PathBuf::from(
        "C:\\Program Files\\Java\\jre8\\bin\\javaw.exe",
    ));
    java_candidates.push(PathBuf::from(
        "C:\\Program Files\\Java\\jre7\\bin\\javaw.exe",
    ));
    java_candidates.push(PathBuf::from(
        "C:\\Program Files\\Java\\jre6\\bin\\javaw.exe",
    ));
    java_candidates.extend(jdk64s);
    java_candidates.extend(new_jdk64s);
    java_candidates.extend(adopt_open_jdk64s);
    java_candidates.extend(foundation_jdk64s);
    java_candidates.extend(adoptium_jdk64s);
    java_candidates.extend(microsoft_jdk64s);
    java_candidates.extend(zulu_64s);
    java_candidates.extend(liberica_64s);

    java_candidates.extend(jre32s);
    java_candidates.extend(new_jre32s);
    java_candidates.extend(adopt_open_jre32s);
    java_candidates.extend(adoptium_jre32s);
    java_candidates.push(PathBuf::from(
        "C:\\Program Files (x86)\\Java\\jre8\\bin\\javaw.exe",
    ));
    java_candidates.push(PathBuf::from(
        "C:\\Program Files (x86)\\Java\\jre7\\bin\\javaw.exe",
    ));
    java_candidates.push(PathBuf::from(
        "C:\\Program Files (x86)\\Java\\jre6\\bin\\javaw.exe",
    ));
    java_candidates.extend(jdk32s);
    java_candidates.extend(new_jdk32s);
    java_candidates.extend(adopt_open_jdk32s);
    java_candidates.extend(foundation_jdk32s);
    java_candidates.extend(adoptium_jdk32s);
    java_candidates.extend(zulu_32s);
    java_candidates.extend(liberica_32s);

    Ok(java_candidates)
}

#[cfg(target_os = "macos")]
fn find_platform_specific_java_candidates() -> anyhow::Result<Vec<PathBuf>> {
    let mut java_candidates = Vec::new();
    java_candidates.push(PathBuf::from("/Applications/Xcode.app/Contents/Applications/Application Loader.app/Contents/MacOS/itms/java/bin/java"));
    java_candidates.push(PathBuf::from(
        "/Library/Internet Plug-Ins/JavaAppletPlugin.plugin/Contents/Home/bin/java",
    ));
    java_candidates.push(PathBuf::from(
        "/System/Library/Frameworks/JavaVM.framework/Versions/Current/Commands/java",
    ));

    match fs::read_dir("/System/Library/Java/JavaVirtualMachines/") {
        Ok(library_jvm_javas) => {
            for java in library_jvm_javas {
                let java = java?;
                java_candidates.push(java.path().join("Contents/Home/bin/java"));
                java_candidates.push(java.path().join("Contents/Commands/java"));
            }
        }
        Err(err) if is_not_found(&err) => {}
        Err(err) => return Err(err.into()),
    }

    java_candidates.push(
        home::home_dir()
            .unwrap_or_default()
            .join(".sdkman/candidates/java"),
    );

    Ok(java_candidates)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn find_platform_specific_java_candidates() -> anyhow::Result<Vec<PathBuf>> {
    let mut java_candidates = Vec::new();

    let mut scan_java_dir = |dir_path: &Path| -> anyhow::Result<()> {
        match fs::read_dir(dir_path) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    java_candidates.push(entry.path().join("jre/bin/java"));
                    java_candidates.push(entry.path().join("bin/java"));
                }
            }
            Err(err) if is_not_found(&err) => {}
            Err(err) => return Err(err.into()),
        }
        Ok(())
    };

    // java installed in a snap is installed in the standard directory, but underneath $SNAP
    let snap = env::var_os("SNAP");
    let mut scan_java_dirs = |dir_path: PathBuf| -> anyhow::Result<()> {
        scan_java_dir(&dir_path)?;
        if let Some(snap) = &snap {
            scan_java_dir(&[Path::new(snap), &dir_path].iter().collect::<PathBuf>())?;
        }
        Ok(())
    };

    // oracle RPMs
    scan_java_dirs(PathBuf::from("/usr/java"))?;
    // general locations used by distro packaging
    scan_java_dirs(PathBuf::from("/usr/lib/jvm"))?;
    scan_java_dirs(PathBuf::from("/usr/lib64/jvm"))?;
    scan_java_dirs(PathBuf::from("/usr/lib32/jvm"))?;
    // manually installed JDKs in /opt
    scan_java_dirs(PathBuf::from("/opt/jdk"))?;
    scan_java_dirs(PathBuf::from("/opt/jdks"))?;
    // flatpak
    scan_java_dirs(PathBuf::from("/app/jdk"))?;

    let home_dir = home::home_dir().unwrap_or_default();

    // javas downloaded by IntelliJ
    scan_java_dirs(home_dir.join(".jdks"))?;
    // javas downloaded by sdkman
    scan_java_dirs(home_dir.join(".sdkman/candidates/java"))?;
    // javas downloaded by gradle (toolchains)
    scan_java_dirs(home_dir.join(".gradle/jdks"))?;

    Ok(java_candidates)
}

fn find_java_paths() -> anyhow::Result<Vec<PathBuf>> {
    let mut java_candidates = find_platform_specific_java_candidates()?;

    java_candidates.extend(get_minecraft_java_bundle()?);
    add_javas_from_env(&mut java_candidates);

    let mut seen_candidates = BTreeSet::new();
    java_candidates
        .into_iter()
        .filter_map(|path| match fs::canonicalize(&path) {
            Err(err) if is_not_found(&err) => None,
            result => Some(result.with_context(|| path.to_string_lossy().into_owned())),
        })
        .filter(|path| match path {
            Ok(path) => seen_candidates.insert(path.clone()),
            Err(_) => true,
        })
        .collect()
}

fn get_minecraft_java_bundle() -> anyhow::Result<Vec<PathBuf>> {
    #[cfg(target_os = "windows")]
    let process_paths = vec![
        [
            &env::var_os("APPDATA").unwrap_or_else(|| std::ffi::OsString::new()),
            std::ffi::OsStr::new(".minecraft"),
            std::ffi::OsStr::new("runtime"),
        ]
        .iter()
        .collect(),
        [
            &env::var_os("LOCALAPPDATA").unwrap_or_else(|| std::ffi::OsString::new()),
            std::ffi::OsStr::new("Packages"),
            std::ffi::OsStr::new("Microsoft.4297127D64EC6_8wekyb3d8bbwe"),
            std::ffi::OsStr::new("LocalCache"),
            std::ffi::OsStr::new("Local"),
            std::ffi::OsStr::new("runtime"),
        ]
        .iter()
        .collect(),
    ];
    #[cfg(target_os = "macos")]
    let process_paths = vec![home::home_dir().unwrap_or_default().join(
        ["Library", "Application Support", "minecraft", "runtime"]
            .iter()
            .collect::<PathBuf>(),
    )];
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let process_paths = vec![home::home_dir()
        .unwrap_or_default()
        .join([".minecraft", "runtime"].iter().collect::<PathBuf>())];

    let mut process_paths = VecDeque::<PathBuf>::from(process_paths);

    let mut javas = Vec::new();
    while let Some(dir_path) = process_paths.pop_front() {
        let entries = match fs::read_dir(dir_path) {
            Ok(entries) => entries.collect::<io::Result<Vec<_>>>()?,
            Err(err) if is_not_found(&err) => continue,
            Err(err) => return Err(err.into()),
        };

        let mut bin_found = false;
        for entry in &entries {
            if entry.file_name() == "bin" {
                javas.push(entry.path().join(JAVA_EXE_NAME));
                bin_found = true;
                break;
            }
        }
        if !bin_found {
            for entry in &entries {
                process_paths.push_back(entry.path());
            }
        }
    }

    Ok(javas)
}

fn add_javas_from_env(java_candidates: &mut Vec<PathBuf>) {
    if let Some(path) = env::var_os("PATH") {
        java_candidates.extend(env::split_paths(&path).map(|path| path.join(JAVA_EXE_NAME)));
    }
    if let Some(java_home) = env::var_os("JAVA_HOME") {
        java_candidates.push(
            [&java_home, OsStr::new("bin"), OsStr::new(JAVA_EXE_NAME)]
                .iter()
                .collect(),
        );
    }
}

fn get_java_version_from_release_file(java_path: &Path) -> anyhow::Result<Option<String>> {
    let Some(parent) = java_path.parent().and_then(|parent| parent.parent()) else {
        return Ok(None);
    };
    let release_file = parent.join("release");
    let release_file = match File::open(release_file) {
        Ok(release_file) => release_file,
        Err(err) if is_not_found(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    for line in BufReader::new(release_file).lines() {
        let line = line?;
        if let Some(version) = line
            .strip_prefix("JAVA_VERSION=\"")
            .and_then(|version| version.strip_suffix('"'))
        {
            return Ok(Some(version.to_owned()));
        }
    }
    Ok(None)
}

fn get_java_version_from_system_property(
    java_path: &Path,
    version_check_dir: &mut Option<TempDir>,
) -> anyhow::Result<String> {
    let version_check_dir = match version_check_dir {
        Some(dir) => dir,
        None => {
            let dir = tempfile::tempdir()?;
            fs::write(
                dir.path().join("VersionCheck.class"),
                include_bytes!("../java_version_check/VersionCheck.class"),
            )?;
            version_check_dir.insert(dir)
        }
    };

    let output = Command::new(java_path)
        .arg("VersionCheck")
        .current_dir(version_check_dir.path())
        .output()?;
    if !output.status.success() {
        bail!(
            "{} returned exit code {} on version check",
            java_path.to_string_lossy(),
            output.status
        )
    }
    Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned())
}

fn get_java_version(
    java_path: &Path,
    version_check_dir: &mut Option<TempDir>,
) -> anyhow::Result<String> {
    match get_java_version_from_release_file(java_path) {
        Ok(Some(version)) => Ok(version),
        Ok(None) => get_java_version_from_system_property(java_path, version_check_dir),
        Err(err) => Err(err),
    }
}

pub fn find_java_candidates() -> anyhow::Result<Vec<JavaCandidate>> {
    let mut version_check_dir = None;
    find_java_paths()?
        .into_iter()
        .map(|path| {
            let version = get_java_version(&path, &mut version_check_dir)?;
            Ok(JavaCandidate { path, version })
        })
        .collect::<anyhow::Result<Vec<_>>>()
}

#[derive(Debug)]
pub struct JavaCandidate {
    path: PathBuf,
    version: String,
}

fn is_not_found(err: &io::Error) -> bool {
    if err.kind() == io::ErrorKind::NotFound {
        return true;
    }

    // TODO: use ErrorKind::NotADirectory once it's stable

    #[cfg(unix)]
    let not_a_directory_error = Some(20);
    #[cfg(windows)]
    let not_a_directory_error = Some(267);
    #[cfg(not(any(unix, windows)))]
    let not_a_directory_error = None;

    let Some(not_a_directory_error) = not_a_directory_error else {
        return false;
    };

    let Some(raw_os_error) = err.raw_os_error() else {
        return false;
    };

    raw_os_error == not_a_directory_error
}