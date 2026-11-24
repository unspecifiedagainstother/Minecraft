use std::collections::HashSet;

#[cfg(target_os = "windows")]
mod imp {
    use super::HashSet;
    use windows::Win32::{
        Foundation::{CloseHandle, BOOL, HWND, LPARAM, TRUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
            IsWindowVisible,
        },
    };

    pub fn calculator_processes() -> anyhow::Result<HashSet<u32>> {
        let mut found = HashSet::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(260);
                    let name =
                        String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                    if name == "calc.exe" || name == "calculatorapp.exe" {
                        found.insert(entry.th32ProcessID);
                    }
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        Ok(found)
    }

    struct EnumData<'a> {
        pids: &'a HashSet<u32>,
        results: Vec<(u32, String)>,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let data = &mut *(lparam.0 as *mut EnumData);
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let title_len = GetWindowTextLengthW(hwnd);
                let title = if title_len > 0 {
                    let mut buf = vec![0u16; (title_len + 1) as usize];
                    GetWindowTextW(hwnd, &mut buf);
                    String::from_utf16_lossy(&buf[..title_len as usize]).into_owned()
                } else {
                    String::new()
                };
                if data.pids.contains(&pid) || title.to_lowercase().contains("calculator") {
                    data.results.push((pid, title));
                }
            }
        }
        TRUE
    }

    pub fn visible_calculator_windows(pids: &HashSet<u32>) -> Vec<(u32, String)> {
        let mut data = EnumData { pids, results: Vec::new() };
        unsafe {
            let _ = EnumWindows(Some(enum_cb), LPARAM(&mut data as *mut _ as isize));
        }
        data.results
    }
}

#[cfg(target_os = "windows")]
pub use imp::{calculator_processes, visible_calculator_windows};

#[cfg(not(target_os = "windows"))]
pub fn calculator_processes() -> anyhow::Result<HashSet<u32>> {
    anyhow::bail!("Windows only")
}

#[cfg(not(target_os = "windows"))]
pub fn visible_calculator_windows(_pids: &HashSet<u32>) -> Vec<(u32, String)> {
    vec![]
}
