"use strict";

const $ = selector => document.querySelector(selector);
const nativeBridge = window.keyai || window.chrome?.webview || {
  postMessage(raw) {
    const message = JSON.parse(raw);
    window.__TAURI__.core.invoke("keyai_handle", { raw }).catch(error => {
      window.__TAURI__.event.emit("keyai-reply", { type: "error", id: message.id || "", text: error?.message || String(error) });
    });
  },
  addEventListener(eventName, callback) {
    if (eventName !== "message" || typeof callback !== "function") return;
    window.__TAURI__.event.listen("keyai-reply", event => callback({ data: event.payload }));
  }
};
const promptBox = $("#prompt");
const feed = $("#feed");
const pendingCalls = new Map();
const modelRequests = new Map();
let callNumber = 0;
let activeMode = "";
let history = [];
let browserTabs = [];
let activeBrowserTab = "";
let browserVisible = false;
let chats = [];
let activeChatId = "";
let workspaceRoot = "";

const modelGroups = {
  openai: "#openaiModels",
  google: "#geminiModels",
  openrouter: "#openrouterModels",
  ollama: "#ollamaModels"
};
const providerNames = {
  openai: "OpenAI API",
  google: "Google AI Studio",
  openrouter: "OpenRouter",
  ollama: "Ollama"
};
const assistantModes = {
  chat: { label: "Обычный чат", instruction: "Помогай с вопросами, планированием и повседневными задачами. Пиши по-русски, если пользователь пишет по-русски." },
  writer: { label: "Сценарист", instruction: "Ты сценарист и редактор сериалов. Помогай с концепцией, персонажами, арками сезона, сериями, сценами и диалогами. Сохраняй внутреннюю логику и голос персонажей; предлагай варианты, когда это полезно." },
  coding: { label: "Кодинг · файлы", instruction: "Ты автономный coding-ассистент. В режиме инструментов сначала изучай рабочую папку, затем реально создавай и меняй нужные файлы. После правок кратко перечисли изменённые файлы. Не заявляй о запуске тестов, если они не запускались." }
};
const modelForProvider = {
  openai: "openai:__refresh__",
  google: "google:__refresh__",
  openrouter: "openrouter:openrouter/free",
  ollama: "ollama:__refresh__"
};

function postNative(message) {
  nativeBridge.postMessage(JSON.stringify(message));
}

function nativeCall(payload, onDelta = null, onProgress = null) {
  return new Promise((resolve, reject) => {
    const id = `req-${++callNumber}`;
    payload.id = id;
    pendingCalls.set(id, { resolve, reject, onDelta, onProgress });
    postNative(payload);
    setTimeout(() => {
      const pending = pendingCalls.get(id);
      if (!pending) return;
      pendingCalls.delete(id);
      pending.reject(new Error("Истекло время ожидания ответа."));
    }, 120000);
  });
}

nativeBridge.addEventListener("message", event => {
  const message = event.data || {};
  if (message.type === "models") {
    renderModels(message.provider, message.models || []);
    if (modelRequests.has(message.id)) {
      modelRequests.delete(message.id);
      showToast(`Каталог ${providerNames[message.provider]} обновлён.`);
    }
    return;
  }
  if (modelRequests.has(message.id)) {
    const provider = modelRequests.get(message.id);
    modelRequests.delete(message.id);
    showToast(formatError(message.text || "Не удалось загрузить каталог моделей.", provider), true);
    return;
  }
  if (message.type === "chatDelta") {
    const pending = pendingCalls.get(message.id);
    if (pending?.onDelta && message.text) pending.onDelta(message.text);
    return;
  }
  if (message.type === "agentProgress") {
    pendingCalls.get(message.id)?.onProgress?.(message.text || "ИИ работает с файлами…");
    return;
  }
  if (message.type === "browserState") {
    updateBrowserState(message);
    return;
  }
  const pending = pendingCalls.get(message.id);
  if (!pending) return;
  pendingCalls.delete(message.id);
  if (message.type === "error") pending.reject(new Error(message.text || "Ошибка API"));
  else pending.resolve(message);
});

function showToast(text, error = false) {
  let toast = $("#toast");
  if (!toast) {
    toast = document.createElement("div");
    toast.id = "toast";
    toast.setAttribute("role", "status");
    document.body.appendChild(toast);
  }
  toast.textContent = text;
  toast.classList.toggle("error", error);
  toast.classList.add("visible");
  clearTimeout(showToast.timer);
  showToast.timer = setTimeout(() => toast.classList.remove("visible"), 5600);
}

function formatError(message, provider) {
  const text = String(message || "");
  const name = providerNames[provider] || "Провайдер";
  if (/429/.test(text)) {
    return `${name} временно ограничил запрос (HTTP 429). Проверьте квоту, баланс и лимиты ключа; можно выбрать другую модель или локальную Ollama.`;
  }
  if (/401|403/.test(text)) {
    return `${name}: ключ неверный, не имеет доступа к этой модели или проекту. Проверьте ключ и права в аккаунте провайдера.`;
  }
  if (/404/.test(text)) return `${name}: модель или API endpoint не найдены. Обновите каталог и выберите доступную модель.`;
  if (/Failed to fetch|connect|подключ|network/i.test(text)) return `${name}: не удалось подключиться. Проверьте сеть и доступность сервиса.`;
  return text;
}

async function readKey(provider) {
  if (provider === "ollama") return "";
  const result = await nativeCall({ type: "key", action: "get", provider });
  return result.key || sessionStorage.getItem(`verdeKey.${provider}`) || "";
}

async function loadModels(provider) {
  try {
    const key = provider === "openrouter" || provider === "ollama" ? "" : await readKey(provider);
    if ((provider === "openai" || provider === "google") && !key) {
      showToast(`Добавьте ключ ${providerNames[provider]} в «Настроить API», чтобы загрузить доступные модели.`, true);
      return;
    }
    const id = `models-${++callNumber}`;
    modelRequests.set(id, provider);
    postNative({ type: "models", provider, key, id });
    setTimeout(() => modelRequests.delete(id), 120000);
  } catch (error) {
    showToast(formatError(error.message, provider), true);
  }
}

function renderModels(provider, models) {
  const group = $(modelGroups[provider]);
  if (!group) return;
  const selectedBefore = $("#model").value;
  group.replaceChildren();
  if (provider === "openrouter") {
    const automatic = document.createElement("option");
    automatic.value = "openrouter:openrouter/free";
    automatic.textContent = "Автовыбор бесплатной модели";
    group.appendChild(automatic);
  }
  if (!models.length) {
    const empty = document.createElement("option");
    empty.disabled = true;
    empty.value = `${provider}:__refresh__`;
    empty.textContent = provider === "ollama"
      ? "Модели не найдены — сначала скачайте модель в Ollama"
      : provider === "openrouter" ? "Каталог пуст" : "Для этого ключа модели не найдены";
    group.appendChild(empty);
    if (selectedBefore.startsWith(`${provider}:`)) $("#model").value = empty.value;
    else if ([...$("#model").options].some(option => option.value === selectedBefore)) $("#model").value = selectedBefore;
    return;
  }
  for (const model of models) {
    const option = document.createElement("option");
    option.value = `${provider}:${model.id}`;
    option.textContent = `${model.name}${model.free ? " · бесплатно" : ""}`;
    group.appendChild(option);
  }
  if (selectedBefore.startsWith(`${provider}:`)) {
    const preferred = provider === "openrouter" ? "openrouter:openrouter/free" : "";
    const target = [...group.options].find(option => option.value === selectedBefore)
      || [...group.options].find(option => option.value === preferred)
      || [...group.options].find(option => !option.disabled);
    if (target) {
      $("#model").value = target.value;
      localStorage.setItem("keyai.model", target.value);
    }
  } else if ([...$("#model").options].some(option => option.value === selectedBefore)) {
    $("#model").value = selectedBefore;
  }
}

function setWorkspace(path) {
  workspaceRoot = path || "";
  localStorage.setItem("keyai.workspace", workspaceRoot);
  const label = $("#workspaceLabel");
  label.textContent = workspaceRoot ? workspaceRoot.replace(/\\/g, " / ") : "Папка не выбрана";
  label.title = workspaceRoot || "Рабочая папка не выбрана";
  $("#termCwd").textContent = localStorage.getItem("keyai.cwd") || workspaceRoot || "Папка не выбрана";
}

async function chooseWorkspace() {
  const result = await nativeCall({ type: "workspace", action: "choose" });
  if (result.path) {
    setWorkspace(result.path);
    localStorage.setItem("keyai.cwd", result.path);
    $("#termCwd").textContent = result.path;
    showToast("Рабочая папка выбрана. Режим «Кодинг · файлы» сможет читать и менять файлы внутри неё.");
  }
  return result.path || "";
}

function selectedModel() {
  const [provider, ...rest] = $("#model").value.split(":");
  return { provider, model: rest.join(":") };
}

function syncEffortControl() {
  const level = Number($("#effort").value) || 2;
  const labels = { 1: "Низкий", 2: "Средний", 3: "Высокий" };
  const selected = selectedModel();
  const disabled = selected.provider === "ollama";
  $("#effortLabel").textContent = labels[level];
  $("#effortToggle").textContent = `${labels[level]}　⌄`;
  $("#effortToggle").disabled = disabled;
  $("#effort").disabled = disabled;
  $("#effortModel").textContent = selected.model || "Уровень рассуждений";
  $("#effortToggle").title = disabled ? "Уровень рассуждений задаёт модель Ollama" : "Уровень рассуждений для выбранной модели";
}

function settings() {
  const current = selectedModel().provider;
  const initialProvider = ["openai", "google", "openrouter", "ollama"].includes(current) ? current : "openrouter";
  $("#modalHost").innerHTML = `<div class="modalback"><div class="modal" role="dialog" aria-modal="true" aria-labelledby="settingsTitle">
    <div class="modalhead"><div><div class="eyebrow">KEYAI 1 · НАСТРОЙКИ</div><h2 id="settingsTitle">Модели и подключение</h2></div><button class="ctl" id="cancel" aria-label="Закрыть">×</button></div>
    <p>Подключите свой аккаунт провайдера. Ключ шифруется Windows DPAPI для текущего пользователя.</p>
    <label class="field">Провайдер<select class="select" id="apiProvider" style="width:100%;margin-top:6px">
      <option value="openai">OpenAI API · GPT‑6 и GPT‑5.6</option><option value="openrouter">OpenRouter · каталог моделей</option><option value="google">Google AI Studio · Gemini</option><option value="ollama">Ollama · локальные модели</option>
    </select></label>
    <label class="field">API endpoint<input id="endpoint" readonly></label>
    <label class="field" id="keyField">API key<input id="key" type="password" autocomplete="off" placeholder="Ключ хранится зашифрованным на этом ПК"></label>
    <div class="notice" id="providerNotice"></div>
    <label class="field">Память KeyAI 1<textarea id="memory" rows="4" placeholder="Предпочтения и контекст для будущих запросов"></textarea></label>
    <div class="notice">Память добавляется как заметки к запросу; модель она не переобучает. Текст запроса отправляется выбранному провайдеру.</div>
    <div class="modalactions"><button class="ctl" id="getKey">Где получить ключ</button><button class="primary" id="saveCfg">Сохранить</button></div>
  </div></div>`;
  const endpoints = {
    openai: "https://api.openai.com/v1/responses",
    openrouter: "https://openrouter.ai/api/v1/chat/completions",
    google: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
    ollama: "http://localhost:11434/v1/chat/completions"
  };
  const keyPages = {
    openai: "https://platform.openai.com/api-keys",
    openrouter: "https://openrouter.ai/settings/keys",
    google: "https://aistudio.google.com/apikey",
    ollama: "https://ollama.com/download/windows"
  };
  let loadGeneration = 0;
  const syncProvider = async () => {
    const provider = $("#apiProvider").value;
    const generation = ++loadGeneration;
    $("#endpoint").value = endpoints[provider];
    $("#keyField").classList.toggle("hidden", provider === "ollama");
    $("#key").value = sessionStorage.getItem(`verdeKey.${provider}`) || "";
    $("#getKey").textContent = provider === "ollama" ? "Установить Ollama" : "Открыть страницу ключей";
    $("#providerNotice").textContent = provider === "openai"
      ? "Используются модели, доступные вашему API-проекту. Оплата API и лимиты управляются отдельно от ChatGPT."
      : provider === "openrouter"
        ? "OpenRouter даёт общий каталог; бесплатные маршруты имеют ограничения провайдера."
        : provider === "google"
          ? "HTTP 429 означает квоту или частотный лимит Google. Доступ зависит от проекта и выбранной модели."
          : "Ollama запускает скачанные модели на этом ПК. Скорость зависит от видеопамяти и размера модели.";
    if (provider === "ollama") return;
    try {
      const key = await readKey(provider);
      if (generation === loadGeneration && provider === $("#apiProvider").value && key) {
        $("#key").value = key;
        sessionStorage.setItem(`verdeKey.${provider}`, key);
      }
    } catch (error) {
      showToast(`Не удалось прочитать сохранённый ключ: ${error.message}`, true);
    }
  };
  $("#apiProvider").value = initialProvider;
  $("#apiProvider").onchange = syncProvider;
  $("#getKey").onclick = () => {
    const url = keyPages[$("#apiProvider").value];
    if ($("#apiProvider").value !== "ollama") $("#modalHost").replaceChildren();
    openBrowser(url);
  };
  $("#cancel").onclick = () => $("#modalHost").replaceChildren();
  $("#saveCfg").onclick = async () => {
    const provider = $("#apiProvider").value;
    try {
      if (provider !== "ollama") {
        const key = $("#key").value.trim();
        await nativeCall({ type: "key", action: "save", provider, key });
        sessionStorage.setItem(`verdeKey.${provider}`, key);
      }
      localStorage.setItem("keyaiMemory", $("#memory").value.trim());
      $("#modalHost").replaceChildren();
      $("#model").value = modelForProvider[provider];
      localStorage.setItem("keyai.model", $("#model").value);
      if (provider !== "ollama") loadModels(provider);
      else loadModels("ollama");
      showToast("Настройки сохранены.");
    } catch (error) {
      showToast(`Не удалось сохранить настройки: ${error.message}`, true);
    }
  };
  $("#memory").value = localStorage.getItem("keyaiMemory") || "";
  syncProvider();
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, ch => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch]);
}

function renderMarkdown(element, source) {
  const blocks = [];
  const links = [];
  let text = String(source || "").replace(/```([\w+#.-]*)[^\S\r\n]*\r?\n([\s\S]*?)```/g, (_, language, code) => {
    const token = `@@BLOCK${blocks.length}@@`;
    blocks.push(`<pre class="codeblock" data-language="${escapeHtml(language || "code")}"><code>${escapeHtml(code.replace(/\n$/, ""))}</code></pre>`);
    return token;
  });
  text = text.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, label, url) => {
    const token = `@@LINK${links.length}@@`;
    links.push(`<a href="#" data-url="${escapeHtml(url)}">${escapeHtml(label)}</a>`);
    return token;
  });
  let html = escapeHtml(text);
  html = html.replace(/`([^`\n]+)`/g, "<code>$1</code>");
  html = html.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/https?:\/\/[^\s<>&"')\]]+/g, url => `<a href="#" data-url="${url}">${url}</a>`);
  html = html.replace(/\n/g, "<br>");
  html = html.replace(/@@BLOCK(\d+)@@/g, (_, index) => blocks[Number(index)] || "");
  html = html.replace(/@@LINK(\d+)@@/g, (_, index) => links[Number(index)] || "");
  element.innerHTML = html;
}

function addMessage(text, who) {
  $("#welcome")?.remove();
  const container = document.createElement("div");
  container.className = who === "user" ? "message" : "reply";
  if (who === "user") container.textContent = text;
  else {
    container.innerHTML = '<div class="mini-orb">K</div><div class="reply-body"><div class="agent-status"></div><div class="replytext"></div></div>';
    renderMarkdown(container.querySelector(".replytext"), text);
  }
  feed.appendChild(container);
  feed.scrollTop = feed.scrollHeight;
  return who === "user" ? container : container.querySelector(".replytext");
}

feed.addEventListener("click", event => {
  const anchor = event.target.closest("a[data-url]");
  if (!anchor) return;
  event.preventDefault();
  openBrowser(anchor.dataset.url);
});

async function send() {
  const text = promptBox.value.trim();
  if (!text) return;
  const selectedMode = $("#assistantMode").value;
  if (selectedMode === "coding" && !workspaceRoot) {
    try {
      const selected = await chooseWorkspace();
      if (!selected) { showToast("Сначала выберите папку, где ИИ сможет работать с файлами.", true); return; }
    } catch (error) { showToast(error.message, true); return; }
  }
  promptBox.value = "";
  const { provider, model } = selectedModel();
  if (!model || model === "__refresh__") {
    promptBox.value = text;
    showToast(`Сначала загрузите каталог моделей ${providerNames[provider]}.`, true);
    loadModels(provider);
    return;
  }
  addMessage(text, "user");
  const output = addMessage("", "assistant");
  const status = output.parentElement.querySelector(".agent-status");
  history.push({ role: "user", content: text });
  const chat = chats.find(item => item.id === activeChatId);
  if (chat) {
    chat.messages.push({ role: "user", content: text });
    if (chat.messages.filter(item => item.role === "user").length === 1) chat.title = text.replace(/\s+/g, " ").slice(0, 48);
    saveChats();
    renderChatList();
    $("#chatCrumb").textContent = chat.title;
  }
  output.innerHTML = '<span class="thinking" aria-label="Модель отвечает"><i></i><i></i><i></i></span>';
  output.closest(".reply").classList.add("busy");
  $("#send").disabled = true;
  try {
    const key = await readKey(provider);
    if (provider !== "ollama" && !key) throw new Error(`Добавьте API key для ${providerNames[provider]} в настройках.`);
    const effort = ({ 1: "low", 2: "medium", 3: "high" })[Number($("#effort").value)] || "medium";
    const memory = localStorage.getItem("keyaiMemory") || "";
    const profile = assistantModes[selectedMode] || assistantModes.chat;
    const system = [
      `Ты — KeyAI 1. Режим: ${profile.label}. ${profile.instruction} Отвечай на языке пользователя, пиши ясно и по делу.`,
      "Если пользователь прикрепил файлы, учитывай их содержимое и называй точные пути. Не утверждай, что запускал код, изменял файлы или проверял сайт, если это не было сделано инструментом.",
      activeMode ? `Текущий режим: ${activeMode}.` : "",
      selectedMode === "coding" ? "В режиме «Кодинг · файлы» используй инструменты для чтения и изменения файлов только внутри выбранной рабочей папки. Если запрос требует правки, внеси её в файлы, а не ограничивайся примером в сообщении. Терминальные команды автоматически не запускай." : "",
      selectedMode === "coding" ? `Выбранная рабочая папка: ${workspaceRoot}. Все пути инструментов относительны к ней.` : "",
      memory ? `Локальные заметки пользователя (это контекст, а не обучение модели):\n${memory}` : ""
    ].filter(Boolean).join("\n\n");
    const messages = [{ role: "system", content: system }, ...history.slice(-30)];
    const body = { model, messages, stream: true };
    if (provider === "openai") body.reasoning_effort = effort;
    else if (provider === "openrouter") body.reasoning = { effort };
    else if (provider === "google") body.reasoning_effort = effort;
    let streamed = "";
    const result = await nativeCall(
      { type: "chat", provider, model, key, agentMode: selectedMode, body: JSON.stringify(body) },
      delta => {
        streamed += delta;
        output.closest(".reply").classList.remove("busy");
        output.textContent = streamed;
        feed.scrollTop = feed.scrollHeight;
      },
      progress => { status.textContent = progress; }
    );
    const answer = result.text || streamed || "Модель вернула пустой ответ.";
    renderMarkdown(output, answer);
    status.textContent = "";
    output.closest(".reply").classList.remove("busy", "error");
    history.push({ role: "assistant", content: answer });
    if (chat) {
      chat.messages.push({ role: "assistant", content: answer });
      saveChats();
    }
  } catch (error) {
    status.textContent = "";
    output.closest(".reply").classList.remove("busy");
    output.closest(".reply").classList.add("error");
    output.textContent = formatError(error.message, provider);
  } finally {
    $("#send").disabled = false;
    promptBox.focus();
  }
}

function makeWelcome() {
  const welcome = document.createElement("div");
  welcome.className = "welcome";
  welcome.id = "welcome";
  welcome.innerHTML = `<div class="orb">K</div><h1>Что будем создавать?</h1><p>KeyAI 1 · код, исследования и ваши файлы</p>
    <div class="cards"><div class="card" data-mode="chat" data-title="Исследование" data-prompt="Исследуй тему и помоги проверить факты по источникам."><b>◎　Исследовать</b><span>Собрать и сверить информацию</span></div>
    <div class="card" data-mode="coding" data-title="Исправление кода" data-prompt="Изучи рабочую папку, найди источник ошибки и исправь её в соответствующих файлах."><b>⌕　Исправить код</b><span>Реальная правка в выбранной папке</span></div>
    <div class="card" data-mode="coding" data-title="Разбор кода" data-prompt="Изучи структуру рабочей папки и объясни назначение ключевых файлов и компонентов."><b>⌘　Разобрать код</b><span>Понять структуру проекта</span></div>
    <div class="card" data-mode="writer" data-title="Сценарий сериала" data-prompt="Помоги придумать и проработать сериал: концепцию, персонажей и арку первого сезона."><b>✧　Сценарий сериала</b><span>Персонажи, сезон, эпизоды и сцены</span></div></div>`;
  feed.appendChild(welcome);
  welcome.querySelectorAll(".card").forEach(card => card.addEventListener("click", () => {
    $("#assistantMode").value = card.dataset.mode;
    setMode(card.dataset.title, card.dataset.prompt);
  }));
}

function setMode(mode, text) {
  activeMode = mode;
  $("#modeLabel").textContent = mode;
  $("#modeLine").classList.remove("hidden");
  promptBox.value = text || "";
  promptBox.focus();
  if (mode === "Исправление кода" || mode === "Разбор кода") setTimeout(() => $("#file").click(), 100);
}

function saveChats() {
  localStorage.setItem("keyai.chats", JSON.stringify(chats.slice(-40)));
}

function renderChatList() {
  const root = $("#recentSessions");
  if (!root) return;
  const query = ($("#sessionSearch")?.value || "").trim().toLowerCase();
  root.replaceChildren();
  chats.slice().reverse().filter(chat => !query || chat.title.toLowerCase().includes(query)).forEach(chat => {
    const item = document.createElement("button");
    item.className = `navitem session-item${chat.id === activeChatId ? " sel" : ""}`;
    item.textContent = chat.title;
    item.title = chat.title;
    item.onclick = () => openChat(chat.id);
    root.appendChild(item);
  });
}

function createChat() {
  activeChatId = `chat-${Date.now()}-${Math.random().toString(16).slice(2, 6)}`;
  chats.push({ id: activeChatId, title: "Новая сессия", messages: [] });
  history = [];
  activeMode = "";
  $("#chatCrumb").textContent = "Новая сессия";
  $("#modeLine").classList.add("hidden");
  feed.replaceChildren();
  makeWelcome();
  saveChats();
  renderChatList();
  promptBox.focus();
}

function openChat(id) {
  const chat = chats.find(item => item.id === id);
  if (!chat) return;
  activeChatId = id;
  activeMode = "";
  $("#modeLine").classList.add("hidden");
  $("#chatCrumb").textContent = chat.title;
  history = chat.messages.slice(-40);
  feed.replaceChildren();
  if (!chat.messages.length) makeWelcome();
  else chat.messages.forEach(message => addMessage(message.content, message.role === "user" ? "user" : "assistant"));
  renderChatList();
}

function browserLayout() {
  requestAnimationFrame(() => {
    const area = $("#webview").getBoundingClientRect();
    postNative({
      type: "browser", action: "bounds", tab: activeBrowserTab,
      visible: browserVisible && !$("#browser").classList.contains("hidden"),
      x: area.left, y: area.top, width: area.width, height: area.height,
      scale: window.devicePixelRatio || 1
    });
  });
}

function setBrowserVisible(value) {
  browserVisible = Boolean(value);
  if (browserVisible) {
    $("#terminal").classList.add("hidden");
    $("#terminalNav").classList.remove("sel");
    $("#codingNav").classList.add("sel");
  }
  $("#browser").classList.toggle("hidden", !browserVisible);
  $(".workspace").classList.toggle("browser-closed", !browserVisible);
  $("#browserToggle").classList.toggle("active", browserVisible);
  browserLayout();
}

function renderBrowserTabs() {
  const strip = $("#tabstrip");
  strip.replaceChildren();
  for (const tab of browserTabs) {
    const item = document.createElement("div");
    item.className = `btab${tab.id === activeBrowserTab ? " active" : ""}`;
    const title = document.createElement("button");
    title.className = "btabtitle";
    title.textContent = tab.title || "Новая вкладка";
    title.title = tab.url || tab.title;
    title.onclick = () => activateBrowserTab(tab.id);
    const close = document.createElement("button");
    close.className = "bclose";
    close.textContent = "×";
    close.title = "Закрыть вкладку";
    close.onclick = event => { event.stopPropagation(); closeBrowserTab(tab.id); };
    item.append(title, close);
    strip.appendChild(item);
  }
  const active = browserTabs.find(tab => tab.id === activeBrowserTab);
  $("#url").value = active?.url || "";
  $("#browserStatus").textContent = active ? (active.url ? "Открыто" : "Новая вкладка") : "Вкладок нет";
  $("#emptyweb").classList.toggle("hidden", Boolean(active?.url));
}

function createBrowserTab(url = "") {
  const id = `browser-${Date.now()}-${Math.random().toString(16).slice(2, 6)}`;
  const tab = { id, title: url ? new URL(url).hostname : "Новая вкладка", url };
  browserTabs.push(tab);
  activeBrowserTab = id;
  setBrowserVisible(true);
  renderBrowserTabs();
  if (url) navigateBrowserTab(id, url);
  return tab;
}

function activateBrowserTab(id) {
  activeBrowserTab = id;
  const tab = browserTabs.find(item => item.id === id);
  renderBrowserTabs();
  if (tab?.url) nativeCall({ type: "browser", action: "activate", tab: id }).catch(error => showToast(error.message, true));
  browserLayout();
}

async function navigateBrowserTab(id, url) {
  const tab = browserTabs.find(item => item.id === id);
  if (!tab) return;
  tab.url = url;
  tab.title = new URL(url).hostname;
  renderBrowserTabs();
  $("#browserStatus").textContent = "Загрузка…";
  browserLayout();
  try {
    await nativeCall({ type: "browser", action: "navigate", tab: id, url });
  } catch (error) {
    $("#browserStatus").textContent = "Ошибка";
    showToast(error.message, true);
  }
}

function closeBrowserTab(id) {
  const index = browserTabs.findIndex(tab => tab.id === id);
  if (index < 0) return;
  browserTabs.splice(index, 1);
  nativeCall({ type: "browser", action: "close", tab: id }).catch(() => {});
  if (activeBrowserTab === id) activeBrowserTab = browserTabs[Math.min(index, browserTabs.length - 1)]?.id || "";
  renderBrowserTabs();
  if (!browserTabs.length) setBrowserVisible(false);
  else if (activeBrowserTab) activateBrowserTab(activeBrowserTab);
  browserLayout();
}

function updateBrowserState(state) {
  const tab = browserTabs.find(item => item.id === state.tab);
  if (!tab) return;
  tab.url = state.url || tab.url;
  tab.title = state.title || (tab.url ? new URL(tab.url).hostname : "Страница");
  if (state.tab === activeBrowserTab) {
    $("#url").value = tab.url;
    $("#browserStatus").textContent = state.success ? "Готово" : "Не удалось открыть страницу";
    $("#emptyweb").classList.add("hidden");
  }
  renderBrowserTabs();
}

function openBrowser(value) {
  let url = String(value || "").trim();
  if (!url) return;
  if (!/^https?:\/\//i.test(url)) {
    url = /\s/.test(url) ? `https://www.google.com/search?q=${encodeURIComponent(url)}` : `https://${url}`;
  }
  try { new URL(url); } catch { showToast("Введите корректный адрес сайта.", true); return; }
  createBrowserTab(url);
}

function handleBrowserAddress() {
  const value = $("#url").value.trim();
  if (!value) return;
  if (!activeBrowserTab) { createBrowserTab(); }
  let url = value;
  if (!/^https?:\/\//i.test(url)) url = /\s/.test(url)
    ? `https://www.google.com/search?q=${encodeURIComponent(url)}` : `https://${url}`;
  navigateBrowserTab(activeBrowserTab, url);
}

function updateBrowserStateFromUrl() {
  const tab = browserTabs.find(item => item.id === activeBrowserTab);
  if (tab) $("#url").value = tab.url;
}

function showTerminal(show = true) {
  browserVisible = false;
  $("#browser").classList.add("hidden");
  $("#terminal").classList.toggle("hidden", !show);
  $(".workspace").classList.toggle("browser-closed", !show);
  $("#browserToggle").classList.remove("active");
  $("#terminalNav").classList.toggle("sel", show);
  $("#codingNav").classList.toggle("sel", !show);
  browserLayout();
  if (show) $("#terminalCommand").focus();
}

function appendTerminalEntry(command, output, exitCode, approved) {
  const root = $("#termOutput");
  root.querySelector(".term-hint")?.remove();
  const row = document.createElement("div");
  row.className = "term-command";
  row.textContent = `PS > ${command}`;
  const result = document.createElement("div");
  result.className = `term-result${approved && exitCode !== 0 ? " error" : ""}`;
  result.textContent = `${output || ""}${approved ? `\n[код завершения: ${exitCode}]` : ""}`;
  root.append(row, result);
  root.scrollTop = root.scrollHeight;
}

async function runTerminalCommand() {
  const input = $("#terminalCommand");
  const command = input.value.trim();
  if (!command) return;
  input.value = "";
  const button = $("#runCommand");
  button.disabled = true;
  button.textContent = "Ожидание…";
  try {
    const result = await nativeCall({
      type: "terminal", code: command,
      cwd: localStorage.getItem("keyai.cwd") || ""
    });
    appendTerminalEntry(command, result.output, result.exitCode, result.approved);
    if (result.cwd) {
      localStorage.setItem("keyai.cwd", result.cwd);
      $("#termCwd").textContent = result.cwd;
    }
  } catch (error) {
    appendTerminalEntry(command, formatError(error.message), -1, true);
  } finally {
    button.disabled = false;
    button.textContent = "Выполнить";
    input.focus();
  }
}

function appendFiles(files) {
  (async () => {
    let total = 0;
    let count = 0;
    const parts = [];
    for (const file of files) {
      if (count >= 12 || total >= 500000) break;
      if (file.size > 100000) continue;
      const name = file.webkitRelativePath || file.name;
      parts.push(`\n\n--- FILE: ${name} ---\n${await file.text()}`);
      total += file.size;
      count++;
    }
    if (parts.length) {
      promptBox.value += `${promptBox.value ? "\n" : ""}${parts.join("")}`;
      promptBox.focus();
    } else showToast("Не найдено текстовых файлов до 100 КБ.", true);
  })();
}

$("#settingsBtn").onclick = $("#settingsTop").onclick = settings;
$("#newChat").onclick = createChat;
$("#send").onclick = send;
$("#workspaceTop").onclick = async () => {
  try { await chooseWorkspace(); }
  catch (error) { showToast(`Не удалось выбрать папку: ${error.message}`, true); }
};
$("#assistantMode").value = localStorage.getItem("keyai.mode") || "chat";
$("#assistantMode").onchange = () => {
  localStorage.setItem("keyai.mode", $("#assistantMode").value);
  activeMode = "";
  $("#modeLine").classList.add("hidden");
};
$("#refreshModels").onclick = () => loadModels(selectedModel().provider);
$("#model").addEventListener("change", () => {
  localStorage.setItem("keyai.model", $("#model").value);
  const selected = selectedModel();
  syncEffortControl();
  if (selected.model === "__refresh__") loadModels(selected.provider);
});
const savedEffort = localStorage.getItem("keyai.effort");
$("#effort").value = savedEffort === "Low" ? "1" : savedEffort === "High" ? "3" : savedEffort === "Medium" ? "2" : savedEffort || "2";
syncEffortControl();
$("#effort").oninput = () => {
  localStorage.setItem("keyai.effort", $("#effort").value);
  syncEffortControl();
};
$("#effortToggle").onclick = () => {
  const open = $("#effortPopover").classList.toggle("hidden") === false;
  $("#effortToggle").setAttribute("aria-expanded", String(open));
};
document.addEventListener("pointerdown", event => {
  if (!event.target.closest(".effort-wrap")) {
    $("#effortPopover").classList.add("hidden");
    $("#effortToggle").setAttribute("aria-expanded", "false");
  }
});
$("#clearMode").onclick = () => { activeMode = ""; $("#modeLine").classList.add("hidden"); };
$("#browserToggle").onclick = () => {
  $("#terminal").classList.add("hidden");
  $("#terminalNav").classList.remove("sel");
  $("#codingNav").classList.add("sel");
  setBrowserVisible(!browserVisible);
};
$("#codingNav").onclick = () => {
  $("#terminal").classList.add("hidden");
  $("#terminalNav").classList.remove("sel");
  $("#codingNav").classList.add("sel");
  setBrowserVisible(false);
};
$("#terminalNav").onclick = () => showTerminal(true);
$("#homeRail").onclick = () => $("#codingNav").click();
$("#historyRail").onclick = $("#titleHistory").onclick = () => {
  $("#sessionSearch").focus();
  $("#sessionSearch").scrollIntoView({ behavior: "smooth", block: "nearest" });
};
$("#toolRail").onclick = () => $("#attach").click();
$("#browserRail").onclick = () => {
  if (!browserVisible) $("#browserToggle").click();
  else setBrowserVisible(false);
};
$("#titleCoding").onclick = () => $("#codingNav").click();
$("#sidebarMenu").onclick = settings;
$("#newBrowserTab").onclick = () => createBrowserTab();
$("#go").onclick = handleBrowserAddress;
$("#url").addEventListener("keydown", event => { if (event.key === "Enter") handleBrowserAddress(); });
$("#attach").onclick = () => $("#file").click();
$("#folderBtn").onclick = () => $("#folder").click();
$("#runCommand").onclick = runTerminalCommand;
$("#terminalCommand").addEventListener("keydown", event => {
  if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); runTerminalCommand(); }
});
$("#chooseCwd").onclick = async () => {
  try {
    const result = await nativeCall({ type: "terminalDirectory" });
    if (result.path) {
      localStorage.setItem("keyai.cwd", result.path);
      $("#termCwd").textContent = result.path;
    }
  } catch (error) { showToast(error.message, true); }
};
$("#clearTerminal").onclick = () => {
  $("#termOutput").replaceChildren();
  $("#termOutput").innerHTML = '<div class="term-hint">Вывод очищен.</div>';
};
$("#file").onchange = event => appendFiles(event.target.files);
$("#folder").onchange = event => appendFiles(event.target.files);
$("#sessionSearch").oninput = renderChatList;
promptBox.addEventListener("keydown", event => {
  if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); send(); }
});
window.addEventListener("resize", browserLayout);
window.addEventListener("load", () => {
  $("#termCwd").textContent = localStorage.getItem("keyai.cwd") || "Папка не выбрана";
  nativeCall({ type: "workspace", action: "get" }).then(result => {
    if (result.path) setWorkspace(result.path);
  }).catch(() => {});
  const savedModel = localStorage.getItem("keyai.model");
  if (savedModel && [...$("#model").options].some(option => option.value === savedModel)) $("#model").value = savedModel;
  try { chats = JSON.parse(localStorage.getItem("keyai.chats") || "[]"); }
  catch { chats = []; }
  if (!Array.isArray(chats)) chats = [];
  if (chats.length) openChat(chats[chats.length - 1].id);
  else createChat();
  renderChatList();
  syncEffortControl();
  loadModels(selectedModel().provider);
});

document.querySelectorAll("[data-window]").forEach(button => button.addEventListener("click", event => {
  event.stopPropagation();
  postNative({ type: "window", action: button.dataset.window });
}));
$("#dragArea").addEventListener("pointerdown", event => {
  if (event.button === 0) postNative({ type: "window", action: "drag" });
});
$("#sessionSearch").value = "";

function insertTitleBarControls() {
  // Retain a useful browser keyboard workflow without giving web pages the app bridge.
}
insertTitleBarControls();
