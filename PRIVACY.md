# Политика конфиденциальности Axiom

Коротко: страницы, история, закладки, пароли и куки хранятся только на вашем компьютере. Сам Axiom отправляет совсем немного, и всё это можно выключить в настройках («Система», подсказки поиска — «Поиск и ИИ», списки фильтров рекламы — «Безопасность»). Об этом же говорит приветствие при первом запуске.

_English version below._

## Что Axiom отправляет сам

**Статистика использования** — на сервер разработчика (`179-254-172-240.sslip.io`).
- Когда: раз в час, пока Axiom открыт, и при запуске.
- Что: версия Axiom (у бета-версии — с номером сборки, например `0.6.3-beta.190`), система (Windows или Linux) и несколько отметок «да/нет»: новая установка; первый запуск за день, неделю, месяц; только что обновился; первый запуск этой версии сегодня; переносили ли данные из другого браузера (один раз, без названия браузера и без того, что перенесено); отметка «в счёт часа», по которой видно, сколько браузеров открыто.
- Чего нет: номера браузера или иного идентификатора, адресов, истории, имени пользователя. По этим данным нельзя узнать конкретного человека.
- Выключить: «Статистика использования». Тогда фоном на сервер разработчика не уходит ничего, в том числе вопрос об ответах на старые сообщения (см. ниже). Ответы на сообщения, которые вы отправили сами, приходят и при выключенной статистике.

**Отчёты об ошибках** — на сервер разработчика.
- Когда: только если Axiom вылетел или упал процесс страницы, при следующем запуске.
- Что: что сломалось и где (сообщение, место в коде, стек), конец журнала `axiom.log`, версия и система. На Linux, если вылет принял systemd-coredump, — ещё имя упавшего процесса Axiom или его движка, сигнал и стек (без командной строки и дампа памяти). Адреса страниц, пути с именем пользователя, почта и токены вычищаются ещё до записи отчёта на диск.
- Выключить: «Отправлять отчёты об ошибках». Тогда отчёты удаляются, никуда не уходя.

**Проверка обновлений** — на GitHub (`github.com`).
- Когда: раз в 6 часов, у бета-версий — раз в полчаса.
- Что: обычный запрос страницы последнего выпуска; в заголовке `User-Agent` — версия Axiom. Обновление скачивается с GitHub только после нажатия «Обновить».
- Выключить: «Проверять автоматически».

**Подсказки поиска** — в выбранную поисковую систему (по умолчанию DuckDuckGo): `duckduckgo.com/ac`, `www.google.com/complete/search`, `suggest.yandex.ru`, `www.bing.com/osjson.aspx`, `search.brave.com/api/suggest`, `www.startpage.com/osuggestions` или `ac.ecosia.org`.
- Когда: пока вы набираете текст в адресной строке или в поиске новой вкладки, ещё до Enter.
- Что: набранный текст (до 120 знаков) и та же строка браузера (`User-Agent`), что у вкладок. Куки не отправляются. Похожее на адрес сайта (`example.com/…`) не отправляется вовсе; со своим адресом поиска подсказок нет.
- Выключить: «Настройки → Поиск и ИИ → Подсказки поисковой системы». Ваши прошлые запросы из истории подсказываются без сети и при выключенных подсказках.

**Списки фильтров рекламы** — с сервера списков EasyList (`easylist-downloads.adblockplus.org`, запасной адрес для EasyList — `easylist.to`).
- Когда: раз в 4 дня (первая проверка — через 20 секунд после запуска), не вышло — через час. И по кнопке «Проверить» в «Настройки → Безопасность».
- Что: обычные запросы файлов `easylist.txt` и `advblock.txt` (RuAdList); в заголовке `User-Agent` — версия Axiom, в `If-Modified-Since` — когда скачан ваш прежний список, чтобы неизменный не качался заново. О вас, о вкладках и об открытых сайтах ничего не уходит: какие запросы блокировать, Axiom решает сам, у себя.
- Выключить: «Настройки → Безопасность → Обновлять списки фильтров». Тогда блокировка работает по спискам, встроенным в Axiom (они обновляются вместе с ним). Сама блокировка рекламы — там же, «Блокировать рекламу»; она ничего не отправляет.

**Ответы на «Сообщить о проблеме»** — на сервер разработчика.
- Когда: только если вы отправляли сообщение, раз в 20 минут в течение месяца после отправки.
- Что: случайный ключ вашего сообщения. С браузером и статистикой он не связан.
- Если Axiom обновился с версии, где ключей ещё не было, он спрашивает и об ответах на старые сообщения. В этом запросе ваших данных нет: сервер присылает проверочные хэши, сверка идёт на вашем компьютере, и только при совпадении браузер отправляет строку своего журнала, которая уже была в том сообщении.

## Что уходит только по вашему действию

- **«Сообщить о проблеме»** — ваш текст, по желанию журнал и файлы. У фото и видео место съёмки и данные камеры вырезаются до отправки.
- **Перевод страниц** — текст страницы уходит в Google Переводчик (`translate.googleapis.com`), когда вы нажали «Перевести» или включили автоперевод для языка. Язык страницы Axiom определяет сам, ничего не отправляя. [Политика Google](https://policies.google.com/privacy).
- **Поиск из адресной строки** — в выбранную поисковую систему (по умолчанию DuckDuckGo). Пока включён «Двойной поиск», запрос уходит ещё и в Google AI Mode в панели ИИ.
- **Сайты, которые вы открываете**, — как в любом браузере.
- **Установка WebView2** (Windows) — если в системе нет WebView2, Axiom спрашивает, установить ли его, и после «Да» скачивает установщик с сайта Microsoft (`go.microsoft.com`) и запускает его.

## Плагины

Плагины ставите вы сами (папка `plugins` в профиле или магазин), и включаются они только вашим переключателем в «Настройки → Плагины» или кнопкой «Установить». Сам Axiom за плагины фоном ничего никуда не отправляет. Но код плагина работает на страницах сайтов, как расширение в Chrome: он видит эти страницы и может обращаться к сети от их имени. Ставьте только плагины, которым доверяете. Хранилище плагина лежит у вас в профиле (`plugin-data`).

**Магазин плагинов** — на сервер магазина (`plugins.179-254-172-240.sslip.io`, отдельно от статистики), только по вашему действию:
- Открыли «Магазин» в «Настройки → Плагины» — Axiom скачивает каталог.
- «Установить» или «Обновить» — скачивается плагин; сервер прибавляет к числу его установок единицу. Ничего о вас и о браузере не уходит, IP-адреса не записываются.
- Выкладывают плагины разработчики на сайте магазина, не из браузера: заявка (имя, как связаться, о себе) и загруженные плагины хранятся на сервере магазина; контакт видит только администратор.

## Движок страниц

На Windows страницы показывает Microsoft Edge WebView2, на Linux — WebKitGTK. Проверка адресов и файлов SmartScreen в движке выключена. Что WebView2 сам отправляет в Microsoft, определяют настройки диагностических данных Windows и [заявление Microsoft о конфиденциальности](https://privacy.microsoft.com/privacystatement).

## Что хранит сервер разработчика

- Статистика — только счётчики по дням, версиям и системам; отметки за последние 48 часов лежат по 10-минутным промежуткам.
- IP-адрес на диск не записывается. Сервер держит его в памяти недолго и только затем, чтобы ограничивать частоту запросов.
- Отчёты и сообщения хранятся 90 дней и не больше 2000 штук. Файлам из «Сообщить о проблеме» отведено 2 ГБ, самые старые удаляются первыми.
- Код сервера открыт: [`server/stats.py`](server/stats.py).

Вопросы — в [Issues](https://github.com/mylifeitsbest/axiom/issues).

---

# Axiom privacy policy (English)

Pages, history, bookmarks, passwords and cookies stay on your computer. Axiom itself sends very little, and every item below can be turned off in Settings (System; search suggestions in Search and AI; ad filter lists in Security). The first-run welcome screen shows the same list with the switches.

**Sent automatically:**
- **Usage statistics** to the developer's server (`179-254-172-240.sslip.io`), hourly while Axiom is open and at start: Axiom version, OS (Windows or Linux) and a few yes/no flags (new install; first run today, this week, this month; just updated; first run of this version today; data imported from another browser, once, without the browser's name or the data; an "hourly" flag used to count open browsers). No identifier, URLs, history or user name. Switch: «Статистика использования» (usage statistics). When it is off, nothing goes to the developer's server in the background except checks for replies to messages you sent yourself.
- **Crash reports** to the developer's server, only after a crash: error message, code location, stack trace, the tail of `axiom.log`, version and OS; on Linux, if systemd-coredump caught the crash, also the name of the crashed Axiom or engine process, the signal and its stack (no command line, no memory dump). URLs, user-name paths, e-mail addresses and tokens are removed before the report is written to disk. Switch: «Отправлять отчёты об ошибках» (send crash reports); when off, reports are deleted unsent.
- **Search suggestions** to the chosen search engine (DuckDuckGo by default) while you type in the address bar or the new-tab search, before you press Enter: the typed text (up to 120 characters) and the same `User-Agent` as the tabs, no cookies. Input that looks like a site address is never sent; a custom search URL gets no suggestions. Switch: «Подсказки поисковой системы» in Settings → Search and AI. Your own past searches from history are suggested offline either way.
- **Update checks** to GitHub, every 6 hours (beta builds: every 30 minutes); the `User-Agent` header carries the Axiom version. Switch: «Проверять автоматически» (check automatically).
- **Ad filter lists** from the EasyList list server (`easylist-downloads.adblockplus.org`, fallback for EasyList `easylist.to`), every 4 days (first check 20 seconds after start, retry in an hour on failure) and on «Проверить» in Settings → Security: plain downloads of `easylist.txt` and `advblock.txt` (RuAdList) with the Axiom version in `User-Agent` and `If-Modified-Since` so an unchanged list is not downloaded again. Nothing about you, your tabs or the sites you open is sent: Axiom decides what to block locally. Switch: «Обновлять списки фильтров» in Settings → Security; when off, the lists built into Axiom are used (they update with Axiom). Ad blocking itself («Блокировать рекламу») sends nothing.
- **Replies to "Report a problem"**: only if you sent a message, every 20 minutes for a month, using that message's random reply key, which is not linked to the browser or to the statistics.

**Sent only when you act:** "Report a problem" (your text, optionally the log and files, with photo and video location and camera data removed); installing WebView2 on Windows when it is missing (after you answer "Yes", the installer is downloaded from Microsoft, `go.microsoft.com`, and started); page translation (page text goes to Google Translate when you click Translate or enable auto-translate for a language); address-bar search (to the chosen search engine, DuckDuckGo by default, plus Google AI Mode in the side panel while "double search" is on); the sites you open.

**Plugins:** you install them yourself (the `plugins` folder in the profile or the plugin store), and each one runs only after you turn it on in Settings → Plugins or press Install. Axiom sends nothing in the background on their behalf, but a plugin's code runs on web pages like a Chrome extension: it sees those pages and can make network requests as them. Install only plugins you trust. A plugin's storage stays in your profile (`plugin-data`).

**Plugin store** (store server `plugins.179-254-172-240.sslip.io`, separate from statistics, only when you act): opening the store downloads the catalog; Install/Update downloads the plugin and the server adds one to its install count (nothing about you or your browser; IP addresses are not written to disk). Developers upload plugins on the store website, not from the browser; their application (name, contact, about) and uploaded plugins are kept on the store server, the contact is seen only by the administrator.

**Engine:** pages are rendered by Microsoft Edge WebView2 on Windows and WebKitGTK on Linux. SmartScreen URL and file checks are disabled in the engine. What WebView2 itself sends to Microsoft is governed by Windows diagnostic data settings and the [Microsoft Privacy Statement](https://privacy.microsoft.com/privacystatement).

**Server storage:** statistics are kept only as counters per day, version and OS (plus 10-minute slots for the last 48 hours). IP addresses are not written to disk; they are held briefly in memory for rate limiting only. Reports and messages are kept for 90 days, at most 2000 of them; attached files share a 2 GB quota, oldest deleted first. The server code is public: [`server/stats.py`](server/stats.py).
