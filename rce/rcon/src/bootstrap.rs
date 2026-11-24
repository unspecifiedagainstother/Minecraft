use anyhow::{bail, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use zip::ZipArchive;

const VERSION: &str = "26.3";
const CLIENT_SHA256: &str =
    "4508d006323f24fa02876310c192d739af56516eb259000ac50f0909a68c9a2d";
const FREETYPE_SHA256: &str =
    "fcac5b5a9a8977f1bc76eccea5e71f6c57250ca5ab8605f06906377e91b6e6b7";
const VERSION_INDEX_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const JAVA_INDEX_URL: &str =
    "https://piston-meta.mojang.com/v1/products/java-runtime/\
     2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

pub struct Manifest {
    pub client_jar: PathBuf,
    pub server_runtime_jar: PathBuf,
    pub server_launch_mode: String,
    pub natives_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub asset_index: String,
    pub classpath_file: PathBuf,
    pub logging_path_file: PathBuf,
    pub options_file: PathBuf,
    pub trusted_servers_dat: PathBuf,
    pub java_path: PathBuf,
}

pub fn sha1_hex(data: &[u8]) -> String {
    use sha1::Digest;
    hex::encode(sha1::Sha1::digest(data))
}

pub fn sha1_hex_file(path: &Path) -> Result<String> {
    Ok(sha1_hex(&std::fs::read(path)?))
}

fn sha256_hex_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    Ok(hex::encode(sha2::Sha256::digest(&std::fs::read(path)?)))
}

fn fetch_json(client: &Client, url: &str) -> Result<Value> {
    Ok(client.get(url).send()?.json()?)
}

fn official_file(
    client: &Client,
    url: &str,
    dest: &Path,
    sha1: Option<&str>,
    size: Option<u64>,
) -> Result<()> {
    if dest.is_file() {
        let size_ok = size.map_or(true, |s| dest.metadata().map_or(false, |m| m.len() == s));
        let hash_ok = sha1.map_or(true, |h| {
            sha1_hex_file(dest).map_or(false, |d| d.eq_ignore_ascii_case(h))
        });
        if size_ok && hash_ok {
            return Ok(());
        }
    }
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = dest.with_extension("tmp_dl");
    let mut last = None;
    for attempt in 0u64..3 {
        match download_file(client, url, &tmp, sha1, size) {
            Ok(()) => {
                std::fs::rename(&tmp, dest)?;
                return Ok(());
            }
            Err(e) => {
                last = Some(e);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_secs(attempt + 1));
                }
            }
        }
    }
    bail!("Could not download {url}: {}", last.unwrap())
}

fn download_file(
    client: &Client,
    url: &str,
    dest: &Path,
    sha1: Option<&str>,
    size: Option<u64>,
) -> Result<()> {
    let mut resp = client
        .get(url)
        .header("User-Agent", "Minecraft-Resource-Pack-PoC/1")
        .send()?;
    let mut file = std::fs::File::create(dest)?;
    io::copy(&mut resp, &mut file)?;
    drop(file);
    if let Some(s) = size {
        if dest.metadata()?.len() != s {
            bail!("Downloaded size mismatch for {url}");
        }
    }
    if let Some(h) = sha1 {
        if !sha1_hex_file(dest)?.eq_ignore_ascii_case(h) {
            bail!("Downloaded checksum mismatch for {url}");
        }
    }
    Ok(())
}

fn nbt_string_bytes(value: &str) -> Vec<u8> {
    let encoded = value.as_bytes();
    let mut out = Vec::new();
    out.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
    out.extend_from_slice(encoded);
    out
}

pub fn write_trusted_servers(path: &Path) -> Result<()> {
    let mut data: Vec<u8> = Vec::new();
    data.extend_from_slice(b"\x0a\x00\x00");
    data.push(0x09);
    data.extend(nbt_string_bytes("servers"));
    data.push(0x0a);
    data.extend_from_slice(&1u32.to_be_bytes());
    data.push(0x01); data.extend(nbt_string_bytes("hidden"));         data.push(0x01);
    data.push(0x08); data.extend(nbt_string_bytes("ip"));             data.extend(nbt_string_bytes("127.0.0.1:25565"));
    data.push(0x08); data.extend(nbt_string_bytes("name"));           data.extend(nbt_string_bytes("Minecraft Server"));
    data.push(0x01); data.extend(nbt_string_bytes("acceptTextures")); data.push(0x01);
    data.push(0x00);
    data.push(0x00);
    std::fs::write(path, data)?;
    Ok(())
}

fn java_version_matches(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    Command::new(path)
        .arg("-version")
        .output()
        .map(|out| {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            combined.contains("version \"25.0.1")
        })
        .unwrap_or(false)
}

fn java_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(h) = std::env::var("JAVA_HOME") {
        v.push(PathBuf::from(h).join("bin").join("java.exe"));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let c = dir.join("java.exe");
            if c.is_file() {
                v.push(c);
                break;
            }
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        v.push(
            PathBuf::from(&appdata)
                .join(".minecraft/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin/java.exe"),
        );
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let packages = PathBuf::from(local).join("Packages");
        if packages.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&packages) {
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with("Microsoft.4297127D64EC6_") {
                        v.push(
                            entry.path()
                                .join("LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin/java.exe"),
                        );
                    }
                }
            }
        }
    }
    v
}

fn official_java(root: &Path, metadata: &Path, client: &Client) -> Result<PathBuf> {
    for candidate in java_candidates() {
        if java_version_matches(&candidate) {
            return Ok(candidate.canonicalize()?);
        }
    }
    println!("Preparing official Mojang Java 25.0.1 runtime...");
    let index: Value = fetch_json(client, JAVA_INDEX_URL)?;
    let epsilon = &index["windows-x64"]["java-runtime-epsilon"];
    let empty = vec![];
    let matches: Vec<&Value> = epsilon
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|item| item["version"]["name"] == "25.0.1")
        .collect();
    if matches.is_empty() {
        bail!("Mojang Java 25.0.1 runtime is unavailable");
    }
    let manifest_info = &matches[0]["manifest"];
    let manifest_path = metadata.join("java-runtime-epsilon-25.0.1.json");
    official_file(
        client,
        manifest_info["url"].as_str().unwrap(),
        &manifest_path,
        manifest_info["sha1"].as_str(),
        manifest_info["size"].as_u64(),
    )?;
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    let runtime_root = root.join("runtime/java-runtime-epsilon");
    std::fs::create_dir_all(&runtime_root)?;
    for (relative, item) in manifest["files"].as_object().unwrap() {
        let target = relative.split('/').fold(runtime_root.clone(), |acc, p| acc.join(p));
        match item["type"].as_str().unwrap_or("") {
            "directory" => { std::fs::create_dir_all(&target)?; }
            "file" => {
                let raw = &item["downloads"]["raw"];
                official_file(
                    client,
                    raw["url"].as_str().unwrap(),
                    &target,
                    raw["sha1"].as_str(),
                    raw["size"].as_u64(),
                )?;
            }
            other => bail!("Unsupported runtime entry: {relative} ({other})"),
        }
    }
    let java_path = runtime_root.join("bin/java.exe");
    if !java_version_matches(&java_path) {
        bail!("Downloaded Mojang runtime is not Java 25.0.1");
    }
    Ok(java_path)
}

fn version_metadata(metadata: &Path, client: &Client) -> Result<Value> {
    let path = metadata.join("26.3.json");
    if !path.is_file() {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let installed = PathBuf::from(appdata)
                .join(format!(".minecraft/versions/{VERSION}/{VERSION}.json"));
            if installed.is_file() {
                std::fs::copy(&installed, &path)?;
            }
        }
    }
    if !path.is_file() {
        let index: Value = fetch_json(client, VERSION_INDEX_URL)?;
        let empty2 = vec![];
        let entries: Vec<&Value> = index["versions"]
            .as_array()
            .unwrap_or(&empty2)
            .iter()
            .filter(|v| v["id"] == VERSION)
            .collect();
        if entries.is_empty() {
            bail!("Minecraft 26.3 is absent from Mojang version metadata");
        }
        official_file(
            client,
            entries[0]["url"].as_str().unwrap(),
            &path,
            entries[0]["sha1"].as_str(),
            None,
        )?;
    }
    let value: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    if value["id"] != VERSION {
        bail!("Unexpected Minecraft version metadata");
    }
    Ok(value)
}

fn library_allowed(library: &Value) -> bool {
    let rules = match library.get("rules").and_then(|r| r.as_array()) {
        Some(r) => r,
        None => return true,
    };
    let mut allowed = false;
    for rule in rules {
        let os = rule["os"]["name"].as_str().unwrap_or("windows");
        if os == "windows" {
            allowed = rule["action"] == "allow";
        }
    }
    allowed
}

fn prepare_assets(root: &Path, version: &Value, client: &Client) -> Result<PathBuf> {
    let asset_id = version["assetIndex"]["id"].as_str().unwrap().to_string();
    let assets = root.join("assets");
    let index_info = &version["assetIndex"];
    let index_path = assets.join("indexes").join(format!("{asset_id}.json"));
    official_file(
        client,
        index_info["url"].as_str().unwrap(),
        &index_path,
        index_info["sha1"].as_str(),
        index_info["size"].as_u64(),
    )?;
    let index: Value = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;

    if let Ok(appdata) = std::env::var("APPDATA") {
        let installed = PathBuf::from(appdata).join(".minecraft/assets");
        let inst_idx = installed.join("indexes").join(format!("{asset_id}.json"));
        if inst_idx.is_file()
            && sha1_hex_file(&inst_idx)
                .map_or(false, |h| h.eq_ignore_ascii_case(index_info["sha1"].as_str().unwrap_or("")))
        {
            let complete = index["objects"]
                .as_object()
                .unwrap()
                .values()
                .all(|item| {
                    let hash = item["hash"].as_str().unwrap();
                    let size = item["size"].as_u64().unwrap();
                    let obj = installed.join("objects").join(&hash[..2]).join(hash);
                    obj.is_file() && obj.metadata().map_or(0, |m| m.len()) == size
                });
            if complete {
                return Ok(installed);
            }
        }
    }

    let mut count = 0usize;
    for item in index["objects"].as_object().unwrap().values() {
        let hash = item["hash"].as_str().unwrap();
        let size = item["size"].as_u64().unwrap();
        let prefix = &hash[..2];
        let target = assets.join("objects").join(prefix).join(hash);
        if !target.is_file() || target.metadata().map_or(0, |m| m.len()) != size {
            official_file(
                client,
                &format!("https://resources.download.minecraft.net/{prefix}/{hash}"),
                &target,
                Some(hash),
                Some(size),
            )?;
        }
        count += 1;
        if count % 1000 == 0 {
            println!("Assets ready: {count}");
        }
    }
    Ok(assets)
}

#[allow(unreachable_code)]
pub fn prepare_dependencies(repo_root: &Path) -> Result<Manifest> {
    #[cfg(not(target_os = "windows"))]
    { bail!("This PoC requires 64-bit Windows"); }

    let root = repo_root.join("dependencies");
    let metadata = root.join("metadata");
    std::fs::create_dir_all(&metadata)?;

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()?;

    println!("Preparing official Minecraft 26.3 files...");
    let version = version_metadata(&metadata, &client)?;

    let client_info = &version["downloads"]["client"];
    let server_info = &version["downloads"]["server"];

    let client_jar = root.join("client/minecraft-26.3-client.jar");
    official_file(
        &client,
        client_info["url"].as_str().unwrap(),
        &client_jar,
        client_info["sha1"].as_str(),
        client_info["size"].as_u64(),
    )?;

    let server_jar = root.join("server/minecraft-26.3-server.jar");
    official_file(
        &client,
        server_info["url"].as_str().unwrap(),
        &server_jar,
        server_info["sha1"].as_str(),
        server_info["size"].as_u64(),
    )?;

    if sha256_hex_file(&client_jar)? != CLIENT_SHA256 {
        bail!("Minecraft client differs from the tested signed 26.3 build");
    }

    let logging_info = &version["logging"]["client"]["file"];
    let logging_id = logging_info["id"].as_str().unwrap();
    let logging = root.join("logging").join(logging_id);
    official_file(
        &client,
        logging_info["url"].as_str().unwrap(),
        &logging,
        logging_info["sha1"].as_str(),
        logging_info["size"].as_u64(),
    )?;

    println!("Preparing libraries and native FreeType...");
    let natives = root.join("natives");
    std::fs::create_dir_all(&natives)?;
    let mut libraries: Vec<PathBuf> = Vec::new();

    for library in version["libraries"].as_array().unwrap() {
        let name = library["name"].as_str().unwrap_or("");
        if !library_allowed(library)
            || name.ends_with(":natives-windows-arm64")
            || name.ends_with(":natives-windows-x86")
        {
            continue;
        }
        let artifact = &library["downloads"]["artifact"];
        let lib_path = artifact["path"]
            .as_str()
            .unwrap()
            .split('/')
            .fold(root.join("libraries"), |acc, p| acc.join(p));
        official_file(
            &client,
            artifact["url"].as_str().unwrap(),
            &lib_path,
            artifact["sha1"].as_str(),
            artifact["size"].as_u64(),
        )?;
        if name.ends_with(":natives-windows") {
            let f = std::fs::File::open(&lib_path)?;
            let mut archive = ZipArchive::new(f)?;
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i)?;
                if entry.name().to_lowercase().ends_with(".dll") {
                    let fname = PathBuf::from(entry.name())
                        .file_name()
                        .unwrap()
                        .to_owned();
                    let mut out = std::fs::File::create(natives.join(&fname))?;
                    io::copy(&mut entry, &mut out)?;
                }
            }
        } else {
            libraries.push(lib_path);
        }
    }

    let freetype = natives.join("freetype.dll");
    if !freetype.is_file() || sha256_hex_file(&freetype)? != FREETYPE_SHA256 {
        bail!("FreeType DLL differs from the tested Minecraft 26.3 build");
    }

    let classpath_file = root.join("classpath.txt");
    let mut cp: Vec<String> = libraries.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    cp.push(client_jar.to_string_lossy().into_owned());
    std::fs::write(&classpath_file, cp.join(";"))?;

    let logging_path_file = root.join("logging-path.txt");
    std::fs::write(&logging_path_file, logging.to_string_lossy().as_ref())?;

    println!("Preparing assets...");
    let assets = prepare_assets(&root, &version, &client)?;
    let java = official_java(&root, &metadata, &client)?;

    let game = root.join("game");
    std::fs::create_dir_all(&game)?;
    let options = game.join("options.txt");
    std::fs::write(
        &options,
        "version:5023\nresourcePacks:[]\nincompatibleResourcePacks:[]\n\
         fullscreen:false\npreferredGraphicsBackend:\"opengl\"\n",
    )?;
    let trusted = game.join("servers.dat");
    write_trusted_servers(&trusted)?;

    println!("Official dependencies ready");

    Ok(Manifest {
        client_jar,
        server_runtime_jar: server_jar,
        server_launch_mode: "jar".to_string(),
        natives_dir: natives,
        assets_dir: assets,
        asset_index: version["assetIndex"]["id"].as_str().unwrap().to_string(),
        classpath_file,
        logging_path_file,
        options_file: options,
        trusted_servers_dat: trusted,
        java_path: java,
    })
}
