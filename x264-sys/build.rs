use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn format_write(builder: bindgen::Builder) -> String {
    builder
        .generate()
        .unwrap()
        .to_string()
        .replace("/**", "/*")
        .replace("/*!", "/*")
}

fn buildver_from_version(version: &str) -> String {
    version
        .split('.')
        .nth(1)
        .unwrap_or(version)
        .to_string()
}

#[cfg(target_os = "windows")]
fn use_pregenerated(out_dir: &Path, buildver: &str) {
    let src = format!("x264-build-{}.rs", buildver);
    let full_src = PathBuf::from("generated").join(src);
    if !full_src.exists() {
        panic!(
            "Expected file \"{}\" not found. Set X264_SYS_GENERATE=1 to generate bindings.",
            full_src.display()
        );
    }
    fs::copy(&full_src, out_dir.join("x264.rs")).unwrap();
}

#[cfg(target_os = "windows")]
fn find_file<F: Fn(&str) -> bool, E: Fn(&Path) -> bool>(
    root: &Path,
    predicate: F,
    exclude: E,
) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if exclude(&dir) {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = match path.file_name().and_then(|v| v.to_str()) {
                Some(name) => name,
                None => continue,
            };
            if predicate(name) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn download_windows_x264(out_dir: &Path) -> (PathBuf, PathBuf) {
    let default_url =
        "https://github.com/ShiftMediaProject/x264/releases/download/0.164.r3194/libx264_0.164.r3194_msvc16.zip";
    let download_url = env::var("X264_DOWNLOAD_URL").unwrap_or_else(|_| default_url.to_string());

    let download_dir = out_dir.join("x264-download");
    let download_stamp = out_dir.join("x264-download.stamp");
    if !download_stamp.exists() {
        if download_dir.exists() {
            fs::remove_dir_all(&download_dir).unwrap();
        }
        fs::create_dir_all(&download_dir).unwrap();

        let zip_path = download_dir.join("x264.zip");
        {
            let mut resp = reqwest::blocking::get(download_url).unwrap();
            assert!(resp.status().is_success());
            let mut out = fs::File::create(&zip_path).unwrap();
            std::io::copy(&mut resp, &mut out).unwrap();
        }

        let mut zip = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap())
            .expect("Failed to read x264 zip file");
        zip.extract(&download_dir)
            .expect("Failed to extract x264 zip file");

        fs::write(&download_stamp, b"downloaded")
            .expect("Failed to write x264 download stamp");
    }

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".to_string());
    let arch_dir = match target_arch.as_str() {
        "x86_64" => "x64",
        "x86" => "x86",
        "aarch64" => "ARM64",
        _ => "x64",
    };

    let lib_pred = |name: &str| {
        name.eq_ignore_ascii_case("libx264.lib") || name.eq_ignore_ascii_case("x264.lib")
    };

    // Try to find in the specific arch directory first
    let lib_path = find_file(
        &download_dir.join("lib").join(arch_dir),
        lib_pred,
        |_| false,
    )
    .or_else(|| {
        // Fallback to recursive search but exclude other arch directories
        find_file(
            &download_dir.join("lib"),
            lib_pred,
            |path| {
                let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
                (name == "x86" && arch_dir != "x86")
                    || (name == "x64" && arch_dir != "x64")
                    || (name == "ARM64" && arch_dir != "ARM64")
            },
        )
    })
    .expect("Downloaded x264 lib file not found");

    let include_path = find_file(
        &download_dir,
        |name| name.eq_ignore_ascii_case("x264.h"),
        |_| false,
    )
    .expect("Downloaded x264 header not found");

    let lib_dir = lib_path
        .parent()
        .expect("x264 lib directory not found")
        .to_path_buf();
    let include_dir = include_path
        .parent()
        .expect("x264 include directory not found")
        .to_path_buf();

    (lib_dir, include_dir)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data/x264.h");
    println!("cargo:rerun-if-changed=generated/x264-build-164.rs");
    println!("cargo:rerun-if-env-changed=X264_LIB_DIR");
    println!("cargo:rerun-if-env-changed=X264_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=X264_VERSION");
    println!("cargo:rerun-if-env-changed=X264_DOWNLOAD_URL");
    println!("cargo:rerun-if-env-changed=X264_SYS_GENERATE");

    let mut include_paths = Vec::new();
    let mut buildver = String::new();
    let mut version = String::new();

    #[cfg(target_os = "windows")]
    let env_lib_dir = env::var_os("X264_LIB_DIR").map(PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let env_lib_dir: Option<PathBuf> = None;

    if let Some(lib_dir) = env_lib_dir {
        version =
            env::var("X264_VERSION").expect("X264_VERSION must be set when X264_LIB_DIR is set");
        buildver = buildver_from_version(&version);

        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        #[cfg(target_os = "windows")]
        println!("cargo:rustc-link-lib=static=libx264");

        let include_dir = env::var_os("X264_INCLUDE_DIR")
            .expect("X264_INCLUDE_DIR must be set when X264_LIB_DIR is set");
        include_paths.push(PathBuf::from(include_dir));
    } else {
        let libs = system_deps::Config::new().probe();
        match libs {
            Ok(libs) => {
                let x264 = libs.get_by_name("x264").unwrap();
                include_paths = x264.include_paths.clone();
                version = x264.version.clone();
                buildver = buildver_from_version(&version);
            }
            Err(err) => {
                #[cfg(target_os = "windows")]
                {
                    let _ = err;
                    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
                    let (lib_dir, include_dir) = download_windows_x264(&out_dir);
                    version = env::var("X264_VERSION").unwrap_or_else(|_| "0.164".to_string());
                    buildver = buildver_from_version(&version);

                    println!("cargo:rustc-link-search=native={}", lib_dir.display());
                    println!("cargo:rustc-link-lib=static=libx264");
                    include_paths.push(include_dir);
                }
                #[cfg(not(target_os = "windows"))]
                {
                    panic!("Failed to find x264 via system-deps: {}", err);
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if env::var_os("X264_SYS_GENERATE").is_none() {
            let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
            use_pregenerated(&out_path, &buildver);
            return;
        }
    }

    let mut builder = bindgen::builder()
        .raw_line(format!(
            "pub unsafe fn x264_encoder_open(params: *mut x264_param_t) -> *mut x264_t {{\n                               x264_encoder_open_{}(params)\n                          }}",
            buildver
        ))
        .header("data/x264.h");

    for header in include_paths {
        builder = builder.clang_arg("-I").clang_arg(header.to_str().unwrap());
    }

    let s = format_write(builder);

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

    let mut file = fs::File::create(out_path.join("x264.rs")).unwrap();

    println!("cargo:version={}", version);

    let _ = file.write(s.as_bytes());
}
