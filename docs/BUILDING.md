# Сборка KeyAI 1 для Windows

## Однокнопочная локальная сборка

1. Установите [Rust через rustup](https://www.rust-lang.org/tools/install) для MSVC target.
2. Установите Visual Studio Build Tools с компонентом **Desktop development with C++**.
3. Скачайте исходники этого каталога и запустите `BUILD-WINDOWS.cmd`.
4. Дождитесь загрузки Rust crates и предварительно собранного `versoview` для Servo/Verso.
5. Ищите установщик `.exe` в `src-tauri\target\release\bundle\nsis\` и ZIP-архив с установщиком в `src-tauri\target\release\bundle\`.

Скрипт проверяет Cargo/Rust и устанавливает `tauri-cli` через Cargo, если его нет. Для сборки нужны интернет и место для кэша Rust crates, браузерного runtime и установщика.

## GitHub Actions

Workflow `.github/workflows/build-windows.yml` запускается вручную, при push в `main` и при теге `v*`. Windows runner устанавливает Rust и NSIS, собирает x64 NSIS installer, упаковывает его в ZIP и сохраняет оба файла как workflow artifacts. Для `v*` workflow прикладывает файлы к GitHub Release.

1. Создайте пустой GitHub-репозиторий.
2. Загрузите в него содержимое папки проекта.
3. Откройте **Actions → Build Windows → Run workflow**.
4. Для релиза отправьте тег, например `v1.1.0`.

Пока CI не завершился зелёным статусом, не распространяйте EXE как проверенную сборку. Для экспериментальной Servo/Verso-версии дополнительно проверьте старт приложения, вкладки, API-ключи и терминал на чистой Windows x64 машине.
