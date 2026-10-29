fn main() {
    tauri_runtime_verso_build::get_verso_as_external_bin().expect("Versoview download failed");
    tauri_build::build();
}
