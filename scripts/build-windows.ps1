$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
$rustc = Get-Command rustc -ErrorAction SilentlyContinue
if (-not $cargo -or -not $rustc) {
    throw 'Не найден Rust/Cargo. Установите Rust через https://www.rust-lang.org/tools/install, затем заново откройте PowerShell.'
}
$hostInfo = (& rustc -vV | Select-String '^host:').ToString()
if ($hostInfo -notmatch 'x86_64-pc-windows-msvc') {
    throw "Ожидался Rust target x86_64-pc-windows-msvc; найдено: $hostInfo. Установите MSVC toolchain и Desktop development with C++."
}
if (-not (Get-Command cargo-tauri -ErrorAction SilentlyContinue)) {
    Write-Host 'Устанавливаю Tauri CLI…' -ForegroundColor Green
    & cargo install tauri-cli --locked
    if ($LASTEXITCODE -ne 0) { throw 'Не удалось установить Tauri CLI.' }
}
Push-Location (Join-Path $projectRoot 'src-tauri')
try {
    & cargo tauri build --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw 'Tauri build завершился с ошибкой.' }
    $installer = Get-ChildItem -Path 'target\release\bundle\nsis' -Filter '*.exe' -File | Select-Object -First 1
    if (-not $installer) { throw 'NSIS installer не найден.' }
    Compress-Archive -LiteralPath $installer.FullName -DestinationPath 'target\release\bundle\KeyAI1-Windows-x64.zip' -Force

    Write-Host "Готово. Файлы находятся в $projectRoot\src-tauri\target\release\bundle" -ForegroundColor Green
} finally {
    Pop-Location
}
