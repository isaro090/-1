# Провайдеры и каталог моделей

KeyAI 1 не содержит копии моделей и не обходит оплату или лимиты. Выберите провайдера, добавьте собственный API key и обновите доступный аккаунту каталог. Имена, цены, скорость, доступ и ограничения определяет провайдер.

| Провайдер | Где получить ключ/модели | Примечание |
|---|---|---|
| OpenAI API | [Ключи OpenAI](https://platform.openai.com/api-keys) · [каталог моделей](https://developers.openai.com/api/docs/models) | Доступные модели зависят от API-проекта. Запросы идут через Responses API. API тарифицируется отдельно от подписки ChatGPT. |
| OpenRouter | [Ключи OpenRouter](https://openrouter.ai/settings/keys) · [модели](https://openrouter.ai/models) | Общий каталог моделей многих поставщиков. Проверяйте карточку, стоимость и условия маршрута. |
| Google AI Studio | [Создать ключ](https://aistudio.google.com/apikey) · [документация Gemini](https://ai.google.dev/gemini-api/docs) | Список получаем из API; HTTP 429 указывает на квоту или частотное ограничение. |
| Ollama | [Ollama для Windows](https://ollama.com/download/windows) · [библиотека моделей](https://ollama.com/library) | Показывает модели, загруженные в локальный Ollama. Производительность зависит от памяти и железа. |

Для OpenAI coding-agent использует Responses API function calling. В официальной документации GPT-6 Astra и GPT-6.1 Sol указаны как модели, для которых вызовы инструментов требуют Responses API. Имена моделей не зашиты: интерфейс получает текущий каталог API-проекта.

Ползунок Low / Medium / High передаёт reasoning effort там, где модель и провайдер поддерживают параметр. Ollama может управлять им локально или игнорировать. Проверяйте доступность, цену и лимиты в кабинете провайдера.
