#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use reqwest::{Client, Response, header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE}};
use serde_json::{Value, json};
use std::{collections::{HashMap, HashSet}, fs, io::Read, path::{Component, Path, PathBuf}, process::{Command, Stdio}, sync::{Arc, Mutex}, thread, time::Duration};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_runtime_verso::VersoRuntime;
use wait_timeout::ChildExt;

struct Core {
    client: Client,
    data_dir: PathBuf,
    workspace: Mutex<PathBuf>,
    browser_views: Mutex<HashMap<String, tauri::Webview<VersoRuntime>>>,
    browser_bounds: Mutex<(f64, f64, f64, f64, bool, String)>,
}

struct Shared(Arc<Core>);

fn emit(app: &AppHandle<VersoRuntime>, value: Value) {
    let _ = app.emit_to("main", "keyai-reply", value);
}

fn emit_error(app: &AppHandle<VersoRuntime>, id: &str, error: impl ToString) {
    emit(app, json!({"type":"error", "id":id, "text":error.to_string()}));
}

fn provider_name(provider: &str) -> &str {
    match provider { "openai" => "OpenAI API", "google" => "Google AI Studio", "openrouter" => "OpenRouter", "ollama" => "Ollama", _ => "провайдер" }
}

fn key_entry(provider: &str) -> Result<keyring::Entry, String> {
    if !["openai", "openrouter", "google"].contains(&provider) { return Err("Неизвестный провайдер API.".into()); }
    keyring::Entry::new("KeyAI1", provider).map_err(|e| e.to_string())
}

fn saved_key(provider: &str) -> Result<String, String> {
    match key_entry(provider)?.get_password() {
        Ok(key) => Ok(key),
        Err(keyring::Error::NoEntry) => Ok(String::new()),
        Err(error) => Err(format!("Не удалось прочитать ключ из Windows Credential Manager: {error}")),
    }
}

fn save_key(provider: &str, key: &str) -> Result<(), String> {
    let entry = key_entry(provider)?;
    if key.trim().is_empty() { return match entry.delete_credential(){Ok(())|Err(keyring::Error::NoEntry)=>Ok(()),Err(error)=>Err(error.to_string())}; }
    entry.set_password(key.trim()).map_err(|e| e.to_string())
}

fn current_workspace(core: &Core) -> PathBuf { core.workspace.lock().unwrap().clone() }

fn set_workspace(core: &Core, path: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !root.is_dir() { return Err("Выберите папку, а не файл.".into()); }
    let saved = serde_json::to_vec(&json!({"path":root.to_string_lossy()})).map_err(|e| e.to_string())?;
    fs::write(core.data_dir.join("workspace.json"), saved).map_err(|e| e.to_string())?;
    *core.workspace.lock().unwrap() = root.clone();
    Ok(root)
}

fn within(root: &Path, path: &Path) -> bool { path == root || path.starts_with(root) }

fn workspace_path(core: &Core, raw: &str, allow_root: bool) -> Result<PathBuf, String> {
    let root = current_workspace(core);
    if root.as_os_str().is_empty() || !root.is_dir() { return Err("Сначала выберите рабочую папку.".into()); }
    let input = Path::new(raw);
    if raw.is_empty() && allow_root { return Ok(root); }
    if raw.trim().is_empty() || input.is_absolute() || input.components().any(|part| !matches!(part, Component::Normal(_))) {
        return Err("Используйте относительный путь без выхода из рабочей папки.".into());
    }
    let path = root.join(input);
    if let Ok(real) = fs::canonicalize(&path) {
        if !within(&root, &real) { return Err("Символическая ссылка выходит за пределы рабочей папки.".into()); }
    } else {
        let mut parent = path.as_path();
        while !parent.exists() {
            parent = parent.parent().ok_or("Не удалось проверить путь.")?;
        }
        if !within(&root, &fs::canonicalize(parent).map_err(|e| e.to_string())?) { return Err("Путь выходит за пределы рабочей папки.".into()); }
    }
    Ok(path)
}

fn relative_workspace(core: &Core, path: &Path) -> Result<String, String> {
    let root = current_workspace(core);
    path.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).map_err(|_| "Путь находится вне рабочей папки.".into())
}

fn history_dir(core: &Core, kind: &str) -> PathBuf {
    let root = current_workspace(core);
    let hash = format!("{:016x}", root.to_string_lossy().to_lowercase().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)));
    core.data_dir.join("workspace-history").join(hash).join(kind)
}

fn backup_file(core: &Core, path: &Path, relative: &str) -> Result<Option<String>, String> {
    if !path.exists() { return Ok(None); }
    if !path.is_file() { return Err("Инструмент работает с файлами, не с папками.".into()); }
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 10_000_000 { return Err("Файл больше 10 МБ; снимок не создан.".into()); }
    let version = format!("{}-{:04x}", chrono_stamp(), rand_suffix());
    let target = history_dir(core, "backups").join(&version).join(relative);
    fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::copy(path, target).map_err(|e| e.to_string())?;
    Ok(Some(version))
}

fn chrono_stamp() -> String {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis().to_string()
}
fn rand_suffix() -> u16 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos() as u16 }

fn log_action(core: &Core, action: &str, path: &str) {
    let record = json!({"at":chrono_stamp(), "action":action, "path":path});
    let file = core.data_dir.join("agent-actions.jsonl");
    let line = format!("{}\n", record);
    let _ = fs::OpenOptions::new().create(true).append(true).open(file).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

fn list_files(root: &Path, base: &Path, depth: usize, found: &mut Vec<String>) -> Result<(), String> {
    let ignored: HashSet<&str> = [".git", "node_modules", ".venv", "venv", "bin", "obj", "dist", "build", ".next", "target"].into_iter().collect();
    if depth > 7 || found.len() >= 1200 { return Ok(()); }
    for item in fs::read_dir(base).map_err(|e| e.to_string())? {
        let entry = item.map_err(|e| e.to_string())?;
        if ignored.contains(entry.file_name().to_string_lossy().as_ref()) || entry.file_type().map_err(|e|e.to_string())?.is_symlink() { continue; }
        let path = entry.path();
        if path.is_dir() { list_files(root, &path, depth + 1, found)?; }
        else if path.is_file() { found.push(path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/")); }
        if found.len() >= 1200 { break; }
    }
    Ok(())
}

fn tool_definitions(responses: bool) -> Value {
    let functions = vec![
        ("list_workspace_files", "List relative file paths under the selected workspace.", json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false})),
        ("read_workspace_file", "Read a UTF-8 file inside the selected workspace.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})),
        ("write_workspace_file", "Create or replace a file. An existing copy is backed up first.", json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false})),
        ("delete_workspace_file", "Move a file to recoverable KeyAI trash; never permanently delete.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})),
        ("list_workspace_versions", "List saved versions for a file. Use when asked to undo or restore.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})),
        ("restore_workspace_file", "Restore a version returned by list_workspace_versions.", json!({"type":"object","properties":{"path":{"type":"string"},"version":{"type":"string"}},"required":["path","version"],"additionalProperties":false}))
    ];
    Value::Array(functions.into_iter().map(|(name,description,parameters)| {
        if responses { json!({"type":"function","name":name,"description":description,"parameters":parameters,"strict":false}) }
        else { json!({"type":"function","function":{"name":name,"description":description,"parameters":parameters}}) }
    }).collect())
}

fn tool_result(core: &Core, name: &str, args: &Value) -> Result<Value, String> {
    let rel = args["path"].as_str().unwrap_or("");
    if name == "list_workspace_files" {
        let root = current_workspace(core);
        let base = workspace_path(core, rel, true)?;
        if !base.is_dir() { return Err("Указанный путь не является папкой.".into()); }
        let mut files = Vec::new();
        list_files(&root, &base, 0, &mut files)?;
        return Ok(json!({"root":root.to_string_lossy(),"files":files,"truncated":files.len() >= 1200}));
    }
    let target = workspace_path(core, rel, false)?;
    let relative = relative_workspace(core, &target)?;
    match name {
        "read_workspace_file" => {
            let meta = fs::metadata(&target).map_err(|e|e.to_string())?;
            if !meta.is_file() { return Err("Указанный путь не является файлом.".into()); }
            if meta.len() > 500_000 { return Err("Файл больше 500 КБ.".into()); }
            let content = fs::read_to_string(&target).map_err(|e|format!("Не удалось прочитать UTF-8 файл: {e}"))?;
            log_action(core,"read",&relative);
            Ok(json!({"path":relative,"content":content}))
        }
        "write_workspace_file" => {
            let content = args["content"].as_str().ok_or("Не передано содержимое файла.")?;
            if content.len() > 1_000_000 { return Err("Новый файл ограничен 1 МБ.".into()); }
            if target.exists() && target.is_dir() { return Err("Указанный путь — папка.".into()); }
            fs::create_dir_all(target.parent().unwrap()).map_err(|e|e.to_string())?;
            let version = backup_file(core,&target,&relative)?;
            fs::write(&target,content).map_err(|e|e.to_string())?;
            let action = if version.is_some() { "write" } else { "create" };
            log_action(core,action,&relative);
            Ok(json!({"path":relative,"result":if version.is_some(){"Файл изменён, предыдущая версия сохранена."}else{"Файл создан."}}))
        }
        "delete_workspace_file" => {
            if !target.is_file() { return Err("Можно переместить только отдельный файл; папки не удаляются.".into()); }
            let version = format!("{}-{:04x}",chrono_stamp(),rand_suffix());
            let trash = history_dir(core,"trash").join(&version).join(&relative);
            fs::create_dir_all(trash.parent().unwrap()).map_err(|e|e.to_string())?;
            fs::copy(&target,&trash).map_err(|e|e.to_string())?;
            fs::remove_file(&target).map_err(|e|e.to_string())?;
            log_action(core,"trash",&relative);
            Ok(json!({"path":relative,"result":"Файл перемещён в восстанавливаемую корзину KeyAI."}))
        }
        "list_workspace_versions" => {
            let mut versions = Vec::new();
            for (folder,kind) in [(history_dir(core,"backups"),"backup"),(history_dir(core,"trash"),"trash")] {
                if let Ok(entries)=fs::read_dir(folder) {
                    for entry in entries.flatten().filter(|e|e.path().is_dir()) {
                        if entry.path().join(&relative).is_file() { versions.push(json!({"version":format!("{}:{}",kind,entry.file_name().to_string_lossy()),"savedAt":entry.file_name().to_string_lossy().split('-').next().unwrap_or("")})); }
                    }
                }
            }
            versions.sort_by(|a,b|b["savedAt"].as_str().unwrap_or("").cmp(a["savedAt"].as_str().unwrap_or("")));
            versions.truncate(30);
            Ok(json!({"path":relative,"versions":versions}))
        }
        "restore_workspace_file" => {
            let version = args["version"].as_str().ok_or("Не указана версия.")?;
            let (kind,id) = version.split_once(':').ok_or("Некорректная версия.")?;
            if !["backup","trash"].contains(&kind) || id.contains("..") || id.contains('/') || id.contains('\\') { return Err("Некорректная версия.".into()); }
            let source = history_dir(core,if kind=="trash"{"trash"}else{"backups"}).join(id).join(&relative);
            if !source.is_file() { return Err("Сохранённая версия не найдена.".into()); }
            backup_file(core,&target,&relative)?;
            fs::create_dir_all(target.parent().unwrap()).map_err(|e|e.to_string())?;
            fs::copy(source,&target).map_err(|e|e.to_string())?;
            log_action(core,"restore",&relative);
            Ok(json!({"path":relative,"result":"Версия восстановлена."}))
        }
        _ => Err(format!("Неизвестный инструмент: {name}"))
    }
}

async fn parse_response(response: Response) -> Result<Value, String> {
    let status = response.status();
    let body = response.text().await.map_err(|e|e.to_string())?;
    let value: Value = serde_json::from_str(&body).unwrap_or_else(|_|json!({"error":{"message":body}}));
    if !status.is_success() { return Err(format!("HTTP {}: {}",status.as_u16(),value["error"]["message"].as_str().unwrap_or("Ошибка API"))); }
    Ok(value)
}

async fn load_models(core: &Core, provider: &str) -> Result<Vec<Value>, String> {
    let root = match provider {
        "openai" => {
            let key=saved_key(provider)?;
            if key.is_empty() { return Err("Добавьте ключ OpenAI в настройках.".into()); }
            parse_response(core.client.get("https://api.openai.com/v1/models").bearer_auth(key).send().await.map_err(|e|e.to_string())?).await?
        }
        "google" => {
            let key=saved_key(provider)?;
            if key.is_empty() { return Err("Добавьте ключ Google AI Studio в настройках.".into()); }
            parse_response(core.client.get("https://generativelanguage.googleapis.com/v1beta/models").header("x-goog-api-key",key).send().await.map_err(|e|e.to_string())?).await?
        }
        "ollama" => parse_response(core.client.get("http://localhost:11434/api/tags").send().await.map_err(|e|e.to_string())?).await?,
        _ => parse_response(core.client.get("https://openrouter.ai/api/v1/models").send().await.map_err(|e|e.to_string())?).await?
    };
    let mut models = Vec::new();
    if provider=="openai" {
        let blocked = ["image","audio","realtime","transcri","whisper","tts","embedding","moderation","dall-e","sora"];
        for item in root["data"].as_array().into_iter().flatten() {
            let id=item["id"].as_str().unwrap_or("");
            let suitable=(id.starts_with("gpt-") || id.starts_with("chatgpt-") || id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4")) && !blocked.iter().any(|x|id.contains(x));
            if suitable { let name=match id {"gpt-6-astra"=>"GPT-6 Astra", "gpt-6.1-sol"=>"GPT-6.1 Sol", "gpt-6.1-terra"=>"GPT-6.1 Terra", "gpt-6-luna"=>"GPT-6 Luna", _=>id}; models.push(json!({"id":id,"name":name,"free":false})); }
        }
    } else if provider=="google" {
        for item in root["models"].as_array().into_iter().flatten() {
            let supported=item["supportedGenerationMethods"].as_array().is_some_and(|a|a.iter().any(|v|v=="generateContent"));
            if supported { let id=item["name"].as_str().unwrap_or("").strip_prefix("models/").unwrap_or(item["name"].as_str().unwrap_or("")); if !id.is_empty(){models.push(json!({"id":id,"name":item["displayName"].as_str().unwrap_or(id),"free":false}));} }
        }
    } else if provider=="ollama" {
        for item in root["models"].as_array().into_iter().flatten(){if let Some(id)=item["name"].as_str(){models.push(json!({"id":id,"name":id,"free":true}));}}
    } else {
        for item in root["data"].as_array().into_iter().flatten(){if let Some(id)=item["id"].as_str(){let free=item["pricing"]["prompt"].as_str()==Some("0")&&item["pricing"]["completion"].as_str()==Some("0");models.push(json!({"id":id,"name":item["name"].as_str().unwrap_or(id),"free":free}));}}
        models.sort_by(|a,b|b["free"].as_bool().cmp(&a["free"].as_bool()).then(a["name"].as_str().cmp(&b["name"].as_str())));
    }
    Ok(models)
}

fn endpoint(provider:&str)->&'static str {
    match provider {"openai"=>"https://api.openai.com/v1/chat/completions", "google"=>"https://generativelanguage.googleapis.com/v1beta/openai/chat/completions", "ollama"=>"http://localhost:11434/v1/chat/completions", _=>"https://openrouter.ai/api/v1/chat/completions"}
}

fn auth_headers(provider:&str,key:&str)->HeaderMap {
    let mut headers=HeaderMap::new();
    headers.insert(CONTENT_TYPE,HeaderValue::from_static("application/json"));
    if provider=="google" { if let Ok(v)=HeaderValue::from_str(key){headers.insert("x-goog-api-key",v);} }
    else if !key.is_empty() { if let Ok(v)=HeaderValue::from_str(&format!("Bearer {key}")){headers.insert(AUTHORIZATION,v);} }
    headers
}

fn tool_progress(app:&AppHandle<VersoRuntime>,id:&str,name:&str,args:&Value) {
    let path=args["path"].as_str().unwrap_or("");
    let text=match name {"list_workspace_files"=>if path.is_empty(){"Смотрю файлы рабочей папки".to_string()}else{format!("Смотрю файлы в {path}")}, "read_workspace_file"=>format!("Читаю {path}"), "write_workspace_file"=>format!("Сохраняю {path}"), "delete_workspace_file"=>format!("Перемещаю {path} в корзину"), "list_workspace_versions"=>format!("Проверяю историю {path}"), "restore_workspace_file"=>format!("Восстанавливаю {path}"), _=>"ИИ работает с файлами…".to_string()};
    emit(app,json!({"type":"agentProgress","id":id,"text":text}));
}

fn function_result(core:&Core,app:&AppHandle<VersoRuntime>,id:&str,name:&str,args:&Value)->Value {
    tool_progress(app,id,name,args);
    match tool_result(core,name,args) {Ok(value)=>value,Err(error)=>json!({"error":error})}
}

async fn chat_json(core:&Core,provider:&str,key:&str,body:&Value)->Result<Value,String>{
    parse_response(core.client.post(endpoint(provider)).headers(auth_headers(provider,key)).json(body).send().await.map_err(|e|e.to_string())?).await
}

async fn responses_json(core:&Core,key:&str,body:&Value)->Result<Value,String>{
    parse_response(core.client.post("https://api.openai.com/v1/responses").bearer_auth(key).json(body).send().await.map_err(|e|e.to_string())?).await
}

async fn responses_chat(core:&Core,message:&Value,key:&str,body:&Value)->Result<String,String>{
    let messages=body["messages"].as_array().cloned().unwrap_or_default();
    let instructions=messages.iter().find(|m|m["role"]=="system").and_then(|m|m["content"].as_str()).unwrap_or("").to_string();
    let input:Vec<Value>=messages.into_iter().filter(|m|m["role"]!="system").collect();
    let mut request=json!({"model":message["model"],"instructions":instructions,"input":input});
    if let Some(effort)=body["reasoning_effort"].as_str(){request["reasoning"]=json!({"effort":effort});}
    let response=responses_json(core,key,&request).await?;
    let text=response["output_text"].as_str().map(str::to_string).unwrap_or_else(||{
        response["output"].as_array().into_iter().flatten().filter(|v|v["type"]=="message").flat_map(|v|v["content"].as_array().into_iter().flatten()).filter_map(|v|v["text"].as_str()).collect::<Vec<_>>().join("")
    });
    Ok(text)
}

fn emit_answer(app:&AppHandle<VersoRuntime>,id:&str,text:&str){
    let chars:Vec<char> = text.chars().collect();
    for part in chars.chunks(220) {
        let value:String=part.iter().collect();
        emit(app,json!({"type":"chatDelta","id":id,"text":value}));
    }
    emit(app,json!({"type":"chatResult","id":id,"text":text}));
}

async fn responses_agent(core:&Core,app:&AppHandle<VersoRuntime>,message:&Value,key:&str,body:&Value,id:&str)->Result<String,String>{
    let messages=body["messages"].as_array().cloned().unwrap_or_default();
    let instructions=messages.iter().find(|m|m["role"]=="system").and_then(|m|m["content"].as_str()).unwrap_or("").to_string();
    let input:Vec<Value>=messages.into_iter().filter(|m|m["role"]!="system").collect();
    let tools=tool_definitions(true);
    let mut request=json!({"model":message["model"],"instructions":instructions,"input":input,"tools":tools,"tool_choice":"auto","parallel_tool_calls":false});
    if let Some(effort)=body["reasoning_effort"].as_str(){request["reasoning"]=json!({"effort":effort});}
    let mut response=responses_json(core,key,&request).await?;
    let mut final_text=String::new();
    for _ in 0..12 {
        let calls:Vec<Value>=response["output"].as_array().into_iter().flatten().filter(|item|item["type"]=="function_call").cloned().collect();
        if calls.is_empty(){
            final_text=response["output_text"].as_str().unwrap_or("").to_string();
            if final_text.is_empty(){final_text=response["output"].as_array().into_iter().flatten().filter(|v|v["type"]=="message").flat_map(|v|v["content"].as_array().into_iter().flatten()).filter_map(|v|v["text"].as_str()).collect::<Vec<_>>().join("");}
            break;
        }
        let mut outputs=Vec::new();
        for call in calls {
            let name=call["name"].as_str().unwrap_or("");
            let args:Value=serde_json::from_str(call["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
            let result=function_result(core,app,id,name,&args);
            outputs.push(json!({"type":"function_call_output","call_id":call["call_id"],"output":result.to_string()}));
        }
        let mut follow=json!({"model":message["model"],"instructions":instructions,"previous_response_id":response["id"],"input":outputs,"tools":tools,"tool_choice":"auto","parallel_tool_calls":false});
        if let Some(effort)=body["reasoning_effort"].as_str(){follow["reasoning"]=json!({"effort":effort});}
        response=responses_json(core,key,&follow).await?;
    }
    if final_text.is_empty(){final_text="Закончил этот шаг работы с файлами. Напишите, если нужно продолжить.".into();}
    Ok(final_text)
}

async fn compatible_agent(core:&Core,app:&AppHandle<VersoRuntime>,message:&Value,key:&str,body:&Value,id:&str)->Result<String,String>{
    let provider=message["provider"].as_str().unwrap_or("openrouter");
    let mut messages=body["messages"].as_array().cloned().unwrap_or_default();
    let tools=tool_definitions(false);
    let mut final_text=String::new();
    for _ in 0..12 {
        let mut request=body.clone();
        request["messages"]=Value::Array(messages.clone()); request["stream"]=json!(false); request["tools"]=tools.clone(); request["tool_choice"]=json!("auto");
        let response=chat_json(core,provider,key,&request).await?;
        let assistant=response["choices"][0]["message"].clone();
        let calls=assistant["tool_calls"].as_array().cloned().unwrap_or_default();
        if calls.is_empty(){final_text=assistant["content"].as_str().unwrap_or("").into();break;}
        messages.push(assistant);
        for call in calls {
            let name=call["function"]["name"].as_str().unwrap_or("");
            let args:Value=serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
            let result=function_result(core,app,id,name,&args);
            messages.push(json!({"role":"tool","tool_call_id":call["id"],"name":name,"content":result.to_string()}));
        }
    }
    if final_text.is_empty(){final_text="Закончил этот шаг работы с файлами. Напишите, если нужно продолжить.".into();}
    Ok(final_text)
}

async fn send_chat(core:Arc<Core>,app:AppHandle<VersoRuntime>,message:Value)->Result<(),String>{
    let id=message["id"].as_str().unwrap_or("");
    let provider=message["provider"].as_str().unwrap_or("openrouter");
    let key=if provider=="ollama"{String::new()}else{saved_key(provider)?};
    if provider!="ollama"&&key.is_empty(){return Err(format!("Добавьте API key для {} в настройках.",provider_name(provider)));}
    let body:Value=serde_json::from_str(message["body"].as_str().unwrap_or("{}" )).map_err(|e|e.to_string())?;
    let answer=if message["agentMode"]=="coding" {
        if current_workspace(&core).as_os_str().is_empty(){return Err("Сначала выберите рабочую папку.".into());}
        if provider=="openai" { responses_agent(&core,&app,&message,&key,&body,id).await? }
        else { compatible_agent(&core,&app,&message,&key,&body,id).await? }
    } else {
        if provider=="openai"{responses_chat(&core,&message,&key,&body).await?}
        else{
            let mut request=body.clone(); request["stream"]=json!(false);
            let response=chat_json(&core,provider,&key,&request).await?;
            response["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string()
        }
    };
    emit_answer(&app,id,&answer);
    Ok(())
}

fn valid_browser_tab(tab:&str)->bool{
    tab.len()>=8 && tab.len()<=80 && tab.starts_with("browser-") && tab.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')
}

fn browser_state(app:&AppHandle<VersoRuntime>,tab:&str,url:&str,success:bool){
    let title=url::Url::parse(url).ok().and_then(|u|u.host_str().map(str::to_string)).unwrap_or_else(||"Страница".into());
    emit(app,json!({"type":"browserState","tab":tab,"url":url,"title":title,"success":success}));
}

fn update_browser_bounds(core:&Core,tab:&str,x:f64,y:f64,width:f64,height:f64,visible:bool,scale:f64){
    let active=if valid_browser_tab(tab){tab.to_string()}else{String::new()};
    let _=scale;
    let bounds=(x,y,width,height,visible,active);
    *core.browser_bounds.lock().unwrap()=bounds.clone();
    let mut views=core.browser_views.lock().unwrap();
    for (label,view) in views.iter_mut(){
        let _=view.set_position(tauri::LogicalPosition::new(bounds.0,bounds.1));
        let _=view.set_size(tauri::LogicalSize::new(bounds.2.max(1.0),bounds.3.max(1.0)));
        if bounds.4&&label==&bounds.5 {let _=view.show();}else{let _=view.hide();}
    }
}

fn browser_action(core:&Core,app:&AppHandle<VersoRuntime>,message:&Value)->Result<(),String>{
    let action=message["action"].as_str().unwrap_or("");
    let tab=message["tab"].as_str().unwrap_or("");
    if !valid_browser_tab(tab){return Err("Некорректный идентификатор вкладки.".into());}
    match action{
        "bounds"=>{
            update_browser_bounds(core,tab,message["x"].as_f64().unwrap_or(0.0),message["y"].as_f64().unwrap_or(0.0),message["width"].as_f64().unwrap_or(0.0),message["height"].as_f64().unwrap_or(0.0),message["visible"].as_bool().unwrap_or(false),message["scale"].as_f64().unwrap_or(1.0));
            Ok(())
        }
        "navigate"=>{
            let raw=message["url"].as_str().ok_or("Не указан адрес страницы.")?;
            let parsed=url::Url::parse(raw).map_err(|_|"Введите корректный адрес страницы.")?;
            if !["http","https"].contains(&parsed.scheme()){return Err("Встроенный браузер открывает только HTTP и HTTPS адреса.".into());}
            let url=parsed.to_string();
            let mut views=core.browser_views.lock().unwrap();
            if let Some(view)=views.get(tab){
                view.navigate(parsed.clone()).map_err(|e|e.to_string())?;
            }else{
                let window=app.get_window("main").ok_or("Не найдено окно приложения.")?;
                let (x,y,w,h,visible,active)=core.browser_bounds.lock().unwrap().clone();
                let builder=tauri::webview::WebviewBuilder::new(tab,tauri::WebviewUrl::External(parsed))
                    .initialization_script("delete window.__TAURI__; delete window.__TAURI_INTERNALS__; delete window.ipc;")
                    .on_navigation(|url|matches!(url.scheme(),"http"|"https"));
                let view=window.add_child(builder,tauri::LogicalPosition::new(x,y),tauri::LogicalSize::new(w.max(1.0),h.max(1.0))).map_err(|e|e.to_string())?;
                if !visible||active!=tab{let _=view.hide();}
                views.insert(tab.to_string(),view);
            }
            drop(views);
            browser_state(app,tab,&url,true);
            Ok(())
        }
        "activate"=>{
            let mut bounds=core.browser_bounds.lock().unwrap();
            bounds.5=tab.to_string();
            let (x,y,w,h,visible)= (bounds.0,bounds.1,bounds.2,bounds.3,true);
            drop(bounds);
            update_browser_bounds(core,tab,x,y,w,h,visible,1.0);
            Ok(())
        }
        "close"=>{
            if let Some(view)=core.browser_views.lock().unwrap().remove(tab){let _=view.close();}
            Ok(())
        }
        _=>Err("Неизвестное действие браузера.".into())
    }
}

fn run_terminal(message:&Value)->Result<Value,String>{
    let code=message["code"].as_str().unwrap_or("").trim();
    if code.is_empty(){return Ok(json!({"approved":false,"output":"","exitCode":0,"cwd":message["cwd"]}));}
    let display:String=if code.chars().count()>5000{format!("{}…",code.chars().take(5000).collect::<String>())}else{code.to_string()};
    let decision=rfd::MessageDialog::new().set_title("KeyAI 1 · подтверждение команды").set_description(format!("Разрешить выполнить эту команду в PowerShell?\n\n{display}")).set_buttons(rfd::MessageButtons::YesNo).show();
    if decision!=rfd::MessageDialogResult::Yes{return Ok(json!({"approved":false,"output":"Команда отменена.","exitCode":0,"cwd":message["cwd"]}));}
    let cwd=message["cwd"].as_str().filter(|p|!p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(||std::env::current_dir().unwrap_or_else(|_|PathBuf::from("C:\\")));
    if !cwd.is_dir(){return Err("Папка терминала не найдена.".into());}
    let mut child=Command::new("powershell.exe").args(["-NoLogo","-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-Command",code]).current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e|format!("Не удалось запустить PowerShell: {e}"))?;
    let stdout=child.stdout.take().ok_or("Не удалось открыть вывод команды.")?;
    let stderr=child.stderr.take().ok_or("Не удалось открыть ошибки команды.")?;
    let out_thread=thread::spawn(move||{let mut text=String::new();let mut limited=stdout.take(1_000_000);let _=limited.read_to_string(&mut text);text});
    let err_thread=thread::spawn(move||{let mut text=String::new();let mut limited=stderr.take(1_000_000);let _=limited.read_to_string(&mut text);text});
    let status=match child.wait_timeout(Duration::from_secs(30)).map_err(|e|e.to_string())?{
        Some(status)=>status,
        None=>{let _=child.kill();let _=child.wait();let _=out_thread.join();let _=err_thread.join();return Ok(json!({"approved":true,"output":"Время выполнения команды превысило 30 секунд; процесс остановлен.","exitCode":-1,"cwd":cwd}));}
    };
    let stdout=out_thread.join().unwrap_or_default();
    let stderr=err_thread.join().unwrap_or_default();
    let mut output=stdout;
    if !stderr.is_empty(){if !output.is_empty(){output.push('\n');}output.push_str(&stderr);}
    if output.len()>1_000_000{output.truncate(1_000_000);output.push_str("\n… вывод сокращён …");}
    Ok(json!({"approved":true,"output":output,"exitCode":status.code().unwrap_or(-1),"cwd":cwd.to_string_lossy()}))
}

#[tauri::command] async fn keyai_handle(raw:String,state:State<'_,Shared>,app:AppHandle<VersoRuntime>,webview:tauri::Webview<VersoRuntime>)->Result<(),String>{
    if webview.label()!="main"{return Err("Команды KeyAI доступны только основному окну приложения.".into());}
    let message:Value=serde_json::from_str(&raw).map_err(|e|format!("Некорректный запрос интерфейса: {e}"))?;
    let id=message["id"].as_str().unwrap_or("");
    let core=state.0.clone();
    match message["type"].as_str().unwrap_or(""){
        "key"=>{
            let provider=message["provider"].as_str().unwrap_or("");
            let result=match message["action"].as_str().unwrap_or(""){
                "get"=>saved_key(provider).map(|key|json!({"type":"keyResult","id":id,"key":key})),
                "save"=>save_key(provider,message["key"].as_str().unwrap_or("")).map(|_|json!({"type":"keyResult","id":id,"saved":true})),
                _=>Err("Неизвестное действие с API-ключом.".into())
            }?;
            emit(&app,result);
        }
        "models"=>{
            let provider=message["provider"].as_str().unwrap_or("openrouter");
            match load_models(&core,provider).await{Ok(models)=>emit(&app,json!({"type":"models","id":id,"provider":provider,"models":models})),Err(error)=>emit_error(&app,id,error)}
        }
        "chat"=>{
            if let Err(error)=send_chat(core,app.clone(),message.clone()).await{emit_error(&app,id,error);}
        }
        "workspace"=>{
            let result:Result<String,String>=match message["action"].as_str().unwrap_or(""){
                "get"=>Ok(current_workspace(&core).to_string_lossy().to_string()),
                "choose"=>match rfd::FileDialog::new().set_title("Выберите рабочую папку для KeyAI 1").pick_folder(){Some(path)=>set_workspace(&core,&path).map(|p|p.to_string_lossy().to_string()),None=>Ok(String::new())},
                _=>Err("Неизвестное действие рабочей папки.".into())
            };
            match result{Ok(path)=>emit(&app,json!({"type":"workspaceResult","id":id,"path":path})),Err(error)=>emit_error(&app,id,error)}
        }
        "terminalDirectory"=>{
            match rfd::FileDialog::new().set_title("Выберите папку терминала").pick_folder(){Some(path)=>emit(&app,json!({"type":"terminalDirectoryResult","id":id,"path":path})),None=>emit(&app,json!({"type":"terminalDirectoryResult","id":id,"path":""}))}
        }
        "terminal"=>match run_terminal(&message){Ok(result)=>emit(&app,json!({"type":"terminalResult","id":id,"output":result["output"],"exitCode":result["exitCode"],"approved":result["approved"],"cwd":result["cwd"]})),Err(error)=>emit_error(&app,id,error)},
        "browser"=>match browser_action(&core,&app,&message){
            Ok(())=>if !id.is_empty(){emit(&app,json!({"type":"browserResult","id":id,"ok":true}));},
            Err(error)=>{
                let tab=message["tab"].as_str().unwrap_or("");
                if message["action"]=="navigate"{browser_state(&app,tab,message["url"].as_str().unwrap_or(""),false);}
                emit_error(&app,id,error);
            }
        },
        "window"=>{
            let window=app.get_window("main").ok_or("Не найдено окно приложения.")?;
            match message["action"].as_str().unwrap_or(""){
                "minimize"=>{let _=window.minimize();},
                "toggleMaximize"=>if window.is_maximized().unwrap_or(false){let _=window.unmaximize();}else{let _=window.maximize();},
                "close"=>{let _=window.close();},
                "drag"=>{let _=window.start_dragging();},
                _=>{}
            }
        }
        _=>emit_error(&app,id,"Неизвестная команда интерфейса."),
    }
    Ok(())
}

fn main(){
    tauri_runtime_verso::builder()
        .manage(Shared(Arc::new(Core{
            client:Client::builder().timeout(Duration::from_secs(120)).build().expect("Не удалось создать HTTP-клиент"),
            data_dir:std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(||std::env::temp_dir()).join("KeyAI1"),
            workspace:Mutex::new(PathBuf::new()),
            browser_views:Mutex::new(HashMap::new()),
            browser_bounds:Mutex::new((0.0,0.0,1.0,1.0,false,String::new()))
        })))
        .setup(|app|{
            let data=std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(||std::env::temp_dir()).join("KeyAI1");
            fs::create_dir_all(&data)?;
            if let Ok(contents)=fs::read(data.join("workspace.json")){
                if let Ok(value)=serde_json::from_slice::<Value>(&contents){
                    if let Some(path)=value["path"].as_str(){if Path::new(path).is_dir(){let _=set_workspace(&app.state::<Shared>().0,Path::new(path));}}
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![keyai_handle])
        .run(tauri::generate_context!())
        .e