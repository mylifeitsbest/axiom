// Пример плагина Axiom. Этот файл выполняется на каждой странице (matches: <all_urls>)
// в изолированном мире плагина: сайт не видит ни этот код, ни объект axiom.

// Кнопка плагина на панели браузера (action в plugin.json).
axiom.on('action', async () => {
  const text = document.body ? document.body.innerText : '';
  const words = (text.match(/[\p{L}\p{N}]+/gu) || []).length;
  const minutes = Math.max(1, Math.round(words / axiom.settings.speed));
  let tail = '';
  if (axiom.settings.counter) {
    // Хранилище плагина: общее для всех сайтов и переживает перезапуск Axiom.
    const clicks = ((await axiom.storage.get('clicks')) || 0) + 1;
    await axiom.storage.set('clicks', clicks);
    tail = ` · нажатие №${clicks}`;
  }
  // Сообщение внизу окна: нужно разрешение "notify".
  await axiom.notify(`${words.toLocaleString('ru-RU')} слов, чтения примерно на ${minutes} мин${tail}`);
});
