/* Расширение веб-процесса WebKitGTK для вкладок: подмена заголовка Referer,
 * запрет сайтам из интернета обращаться к устройствам локальной сети и счётчик
 * заблокированной рекламы.
 *
 * Заголовки и сами запросы WebKitGTK даёт трогать только внутри веб-процесса —
 * в сигнале WebKitWebPage::send-request. build.rs собирает этот файл в .so, Axiom
 * носит его в себе и кладёт в профиль перед первой вкладкой (src/tabs/identity.rs).
 *
 * Выбор приходит от Axiom: при запуске процесса — в данных инициализации
 * "(usat)" (режим, свой адрес, id страниц вкладок), дальше — сообщениями всем
 * процессам: "axiom-referer" "(us)" и "axiom-tab" "(t)" (id страницы новой вкладки).
 * Страницы интерфейса и панели ИИ живут в тех же процессах — их Referer не трогаем,
 * а запрет на локальную сеть действует для всех страниц.
 */
#include <gmodule.h>
#include <string.h>
#include <webkit2/webkit-web-extension.h>

/* Режимы — как referer_mode в src/tabs/identity.rs. */
enum { KEEP = 0, OFF = 1, SITE = 2, CUSTOM = 3 };

static guint mode = KEEP;
static gchar *custom;
/* id страниц, которые вкладки (gint64 → он же). */
static GHashTable *tabs;

static void set_referer(guint32 next, const gchar *value)
{
    g_free(custom);
    custom = g_strdup(value);
    mode = next <= CUSTOM ? next : KEEP;
    if (mode == CUSTOM && !*custom)
        mode = KEEP;
}

static void add_tab(guint64 id)
{
    if (g_hash_table_contains(tabs, &id))
        return;
    gint64 *key = g_new(gint64, 1);
    *key = (gint64)id;
    g_hash_table_add(tabs, key);
}

/* Адрес сайта запроса: «https://host[:port]/». Не http(s) — NULL. */
static gchar *site_of(const gchar *uri)
{
    GUri *parsed = uri ? g_uri_parse(uri, G_URI_FLAGS_NONE, NULL) : NULL;
    if (!parsed)
        return NULL;
    const gchar *scheme = g_uri_get_scheme(parsed);
    const gchar *host = g_uri_get_host(parsed);
    gint port = g_uri_get_port(parsed);
    gchar *site = NULL;
    if (scheme && host && *host
        && (!g_ascii_strcasecmp(scheme, "http") || !g_ascii_strcasecmp(scheme, "https"))) {
        /* Адрес IPv6 GUri отдаёт без скобок. */
        gboolean v6 = strchr(host, ':') != NULL;
        const gchar *open = v6 ? "[" : "", *close = v6 ? "]" : "";
        site = port > 0 ? g_strdup_printf("%s://%s%s%s:%d/", scheme, open, host, close, port)
                        : g_strdup_printf("%s://%s%s%s/", scheme, open, host, close);
    }
    g_uri_unref(parsed);
    return site;
}

/* ---------- локальная сеть ----------
 *
 * Страница из интернета без единого клика могла слать запросы роутеру, принтеру,
 * dev-серверу на 127.0.0.1 — подделывать формы (CSRF) и перебирать порты. Chromium
 * и Firefox такое закрывают (Private / Local Network Access), WebKitGTK — нет.
 * Поэтому как в Chromium: адреса делятся на «этот компьютер», «локальная сеть»
 * и «интернет», и страница не может обратиться к пространству ближе своего —
 * сайт из интернета к локальной сети и к компьютеру, сайт из локальной сети
 * к компьютеру. Сайт определяется по адресу вкладки, поэтому и `data:`-фрейм
 * внутри неё не обойдёт запрет. Переходы самой вкладки (набрали адрес роутера,
 * страница входа в сеть отеля перенаправила на 10.0.0.1) не запрещаются.
 *
 * Смотрим на адрес в ссылке: IP, `localhost`, `*.local`, имя без точки. Имя,
 * которое DNS превратит в локальный адрес, отсюда не видно — до соединения
 * его ещё никто не разрешил. */

enum { LOOPBACK = 0, PRIVATE = 1, PUBLIC = 2, UNKNOWN = 3 };

/* Хост из адреса http(s)/ws(s) в том виде, как его привёл WebKit:
 * «схема://[кто@]хост[:порт][/?#…]», у `blob:` — его создатель. Скобки IPv6
 * снимаем. Не такой адрес — NULL. GUri здесь не годится: адрес с кривым `%zz`
 * он не разбирает, а WebKit такой запрос отправит. */
static gchar *host_of(const gchar *uri)
{
    static const gchar *const schemes[] = {"http://", "https://", "ws://", "wss://"};
    if (!uri)
        return NULL;
    if (!g_ascii_strncasecmp(uri, "blob:", 5))
        uri += 5;
    const gchar *rest = NULL;
    for (gsize i = 0; i < G_N_ELEMENTS(schemes) && !rest; i++) {
        gsize n = strlen(schemes[i]);
        if (!g_ascii_strncasecmp(uri, schemes[i], n))
            rest = uri + n;
    }
    if (!rest)
        return NULL;
    gsize end = strcspn(rest, "/?#\\");
    const gchar *host = rest;
    for (gsize i = 0; i < end; i++)
        if (rest[i] == '@')
            host = rest + i + 1;
    gsize len = end - (gsize)(host - rest);
    if (len && host[0] == '[') {
        const gchar *close = memchr(host, ']', len);
        return close && close > host + 1 ? g_strndup(host + 1, (gsize)(close - host - 1)) : NULL;
    }
    const gchar *colon = memchr(host, ':', len);
    if (colon)
        len = (gsize)(colon - host);
    return len ? g_strndup(host, len) : NULL;
}

/* 10/8, 172.16/12, 192.168/16, 169.254/16 (link-local), 100.64/10 (CGNAT). */
static gboolean private_v4(const guint8 *b)
{
    return b[0] == 10 || (b[0] == 172 && (b[1] & 0xf0) == 16) || (b[0] == 192 && b[1] == 168)
        || (b[0] == 169 && b[1] == 254) || (b[0] == 100 && (b[1] & 0xc0) == 64);
}

static guint space_of_address(GInetAddress *address)
{
    static const guint8 mapped[12] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff};
    const guint8 *b = g_inet_address_to_bytes(address);
    gsize n = g_inet_address_get_native_size(address);
    /* ::ffff:a.b.c.d — тот же IPv4. */
    if (n == 16 && !memcmp(b, mapped, sizeof mapped)) {
        b += 12;
        n = 4;
    }
    if (n == 4) {
        /* 0.0.0.0 на Linux — тоже этот компьютер. */
        if (b[0] == 127 || b[0] == 0)
            return LOOPBACK;
        return private_v4(b) ? PRIVATE : PUBLIC;
    }
    if (n != 16)
        return UNKNOWN;
    if (g_inet_address_get_is_loopback(address) || g_inet_address_get_is_any(address))
        return LOOPBACK;
    /* fc00::/7 (ULA), fe80::/10 (link-local), fec0::/10 (старый site-local). */
    if ((b[0] & 0xfe) == 0xfc || (b[0] == 0xfe && (b[1] & 0x80) == 0x80))
        return PRIVATE;
    return PUBLIC;
}

static guint space_of_host(const gchar *raw)
{
    if (!raw || !*raw)
        return UNKNOWN;
    gchar *host = g_ascii_strdown(raw, -1);
    gsize len = strlen(host);
    while (len && host[len - 1] == '.')
        host[--len] = '\0';
    guint space;
    GInetAddress *address = len ? g_inet_address_new_from_string(host) : NULL;
    if (!len) {
        space = UNKNOWN;
    } else if (address) {
        space = space_of_address(address);
    } else if (!strcmp(host, "localhost") || g_str_has_suffix(host, ".localhost")) {
        space = LOOPBACK;
    } else if (g_str_has_suffix(host, ".local") || !strchr(host, '.')) {
        /* mDNS (`printer.local`) и имена без точки (`http://router/`) — это сеть рядом. */
        space = PRIVATE;
    } else {
        space = PUBLIC;
    }
    if (address)
        g_object_unref(address);
    g_free(host);
    return space;
}

static guint space_of_uri(const gchar *uri)
{
    gchar *host = host_of(uri);
    guint space = space_of_host(host);
    g_free(host);
    return space;
}

/* Страница с адресом `page` не может запросить `target`. */
static gboolean crosses_into_local(const gchar *page, const gchar *target)
{
    guint from = space_of_uri(page);
    if (from == UNKNOWN || from == LOOPBACK)
        return FALSE;
    guint to = space_of_uri(target);
    return to != UNKNOWN && to < from;
}

#ifndef AXIOM_WEBEXT_TEST

/* Перенаправление загрузки самой вкладки: она уходит со страницы, которую
 * показывает (адрес вкладки WebKit уже сменил на новый, а открыт ещё старый). */
static gboolean own_redirect(WebKitWebPage *page, WebKitURIResponse *redirect,
                             const gchar *page_uri)
{
    if (!redirect || !page_uri)
        return FALSE;
    WebKitFrame *frame = webkit_web_page_get_main_frame(page);
    const gchar *shown = frame ? webkit_frame_get_uri(frame) : NULL;
    return !g_strcmp0(webkit_uri_response_get_uri(redirect), page_uri)
        && g_strcmp0(shown, page_uri);
}

/* Запрос из вкладки к локальной сети, которого быть не должно. */
static gboolean blocked(WebKitWebPage *page, WebKitURIRequest *request,
                        WebKitURIResponse *redirect)
{
    static guint told;
    const gchar *page_uri = webkit_web_page_get_uri(page);
    const gchar *target = webkit_uri_request_get_uri(request);
    if (!crosses_into_local(page_uri, target) || own_redirect(page, redirect, page_uri))
        return FALSE;
    /* В журнал — первые случаи: перебор портов дал бы тысячи строк. */
    if (told < 20) {
        told++;
        g_message("Axiom: запрос к локальной сети запрещён: %s (со страницы %s)", target,
                  page_uri);
    }
    return TRUE;
}

static gboolean on_send_request(WebKitWebPage *page, WebKitURIRequest *request,
                                WebKitURIResponse *redirect, gpointer data)
{
    (void)data;
    /* Локальная сеть — для любой страницы, а не только для вкладок из списка:
     * вкладка попадает в него сообщением от Axiom, а страница могла начать грузиться
     * раньше, и её первые запросы ушли бы без проверки. Так же и панель ИИ: в списке
     * её нет никогда. Интерфейс (`axiom://`) правило и так не трогает. TRUE — запрос
     * не уходит. */
    if (blocked(page, request, redirect))
        return TRUE;
    guint64 id = webkit_web_page_get_id(page);
    if (!g_hash_table_contains(tabs, &id))
        return FALSE;
    if (mode == KEEP)
        return FALSE;
    SoupMessageHeaders *headers = webkit_uri_request_get_http_headers(request);
    if (!headers)
        return FALSE;
    if (mode == OFF) {
        soup_message_headers_remove(headers, "Referer");
    } else if (mode == CUSTOM) {
        soup_message_headers_replace(headers, "Referer", custom);
    } else {
        gchar *site = site_of(webkit_uri_request_get_uri(request));
        if (site)
            soup_message_headers_replace(headers, "Referer", site);
        g_free(site);
    }
    return FALSE;
}

/* ---------- блокировка рекламы: счётчик ----------
 *
 * Рекламу блокирует сам WebKit по правилам Axiom (src/adblock/linux.rs) и о каждой
 * заблокированной загрузке пишет в консоль страницы. По этим строкам вкладка
 * узнаёт, сколько заблокировано: ей уходит сообщение "axiom-adblock". Сигнал
 * консоли в WebKitGTK 2.40 объявлен устаревшим, но в API 4.1 он есть и работает. */

#define BLOCKED_PREFIX "Content blocker prevented frame displaying "

G_GNUC_BEGIN_IGNORE_DEPRECATIONS
static void on_console(WebKitWebPage *page, WebKitConsoleMessage *message, gpointer data)
{
    (void)data;
    const gchar *text = webkit_console_message_get_text(message);
    if (!text || !g_str_has_prefix(text, BLOCKED_PREFIX))
        return;
    guint64 id = webkit_web_page_get_id(page);
    if (!g_hash_table_contains(tabs, &id))
        return;
    webkit_web_page_send_message_to_view(page, webkit_user_message_new("axiom-adblock", NULL), NULL,
                                         NULL, NULL);
}
G_GNUC_END_IGNORE_DEPRECATIONS

static void on_page_created(WebKitWebExtension *extension, WebKitWebPage *page, gpointer data)
{
    (void)extension;
    (void)data;
    g_signal_connect(page, "send-request", G_CALLBACK(on_send_request), NULL);
    g_signal_connect(page, "console-message-sent", G_CALLBACK(on_console), NULL);
}

static gboolean on_message(WebKitWebExtension *extension, WebKitUserMessage *message,
                           gpointer data)
{
    (void)extension;
    (void)data;
    const gchar *name = webkit_user_message_get_name(message);
    GVariant *params = webkit_user_message_get_parameters(message);
    if (!params)
        return FALSE;
    if (!g_strcmp0(name, "axiom-referer") && g_variant_is_of_type(params, G_VARIANT_TYPE("(us)"))) {
        guint32 next;
        const gchar *value;
        g_variant_get(params, "(u&s)", &next, &value);
        set_referer(next, value);
        return TRUE;
    }
    if (!g_strcmp0(name, "axiom-tab") && g_variant_is_of_type(params, G_VARIANT_TYPE("(t)"))) {
        guint64 id;
        g_variant_get(params, "(t)", &id);
        add_tab(id);
        return TRUE;
    }
    return FALSE;
}

G_MODULE_EXPORT void webkit_web_extension_initialize_with_user_data(WebKitWebExtension *extension,
                                                                    GVariant *data)
{
    tabs = g_hash_table_new_full(g_int64_hash, g_int64_equal, g_free, NULL);
    custom = g_strdup("");
    if (data && g_variant_is_of_type(data, G_VARIANT_TYPE("(usat)"))) {
        guint32 next;
        const gchar *value;
        GVariantIter *ids;
        guint64 id;
        g_variant_get(data, "(u&sat)", &next, &value, &ids);
        set_referer(next, value);
        while (g_variant_iter_next(ids, "t", &id))
            add_tab(id);
        g_variant_iter_free(ids);
    }
    g_signal_connect(extension, "page-created", G_CALLBACK(on_page_created), NULL);
    g_signal_connect(extension, "user-message-received", G_CALLBACK(on_message), NULL);
}

#else /* AXIOM_WEBEXT_TEST: проверка правил локальной сети без WebKit (CI, Linux). */

#include <stdio.h>

int main(void)
{
    static const struct {
        const gchar *page, *target;
        gboolean blocked;
    } cases[] = {
        /* Интернет → компьютер и локальная сеть. */
        {"https://evil.example/", "http://127.0.0.1:18906/secret", TRUE},
        {"https://evil.example/", "http://localhost:8080/", TRUE},
        {"https://evil.example/", "http://api.localhost./", TRUE},
        {"https://evil.example/", "http://[::1]:631/", TRUE},
        {"https://evil.example/", "http://[::ffff:127.0.0.1]/", TRUE},
        {"https://evil.example/", "http://0.0.0.0:3000/", TRUE},
        {"https://evil.example/", "http://192.168.0.1/apply.cgi", TRUE},
        {"https://evil.example/", "http://10.1.2.3/", TRUE},
        {"https://evil.example/", "http://172.31.0.1/", TRUE},
        {"https://evil.example/", "http://169.254.169.254/latest/", TRUE},
        {"https://evil.example/", "http://100.100.1.1/", TRUE},
        {"https://evil.example/", "http://[fd00::1]/", TRUE},
        {"https://evil.example/", "http://[fe80::1]/", TRUE},
        {"https://evil.example/", "http://printer.local/", TRUE},
        {"https://evil.example/", "http://router/", TRUE},
        {"https://evil.example/", "ws://127.0.0.1:9222/devtools", TRUE},
        {"https://evil.example/", "http://user@192.168.0.1:80/%zz", TRUE},
        {"https://evil.example/", "http://a@b@192.168.0.1/", TRUE},
        {"blob:https://evil.example/1-2", "http://192.168.0.1/", TRUE},
        {"https://EVIL.example/", "HTTP://LOCALHOST/", TRUE},
        /* Локальная сеть → компьютер. */
        {"http://192.168.0.113:18905/attack.html", "http://127.0.0.1:18906/", TRUE},
        /* Разрешено: в своё пространство и дальше от себя. */
        {"https://evil.example/", "https://cdn.example/x.js", FALSE},
        {"https://evil.example/", "http://172.32.0.1/", FALSE},
        {"https://evil.example/", "http://11.0.0.1/", FALSE},
        {"https://evil.example/", "http://100.128.0.1/", FALSE},
        {"https://evil.example/", "http://[2001:db8::1]/", FALSE},
        {"http://192.168.0.1/", "http://192.168.0.1/style.css", FALSE},
        {"http://192.168.0.1/", "http://10.0.0.5/", FALSE},
        {"http://192.168.0.1/", "https://cdn.example/x.js", FALSE},
        {"http://localhost:3000/", "http://127.0.0.1:8080/api", FALSE},
        {"http://localhost:3000/", "http://192.168.0.1/", FALSE},
        {"http://127.0.0.1:18905/", "http://127.0.0.1:18906/", FALSE},
        /* Не сайт — не трогаем: интерфейс, файлы, пустая вкладка, кривой адрес. */
        {"axiom://localhost/", "http://192.168.0.1/favicon.ico", FALSE},
        {"file:///home/u/a.html", "http://127.0.0.1/", FALSE},
        {"about:blank", "http://127.0.0.1/", FALSE},
        {NULL, "http://127.0.0.1/", FALSE},
        {"https://evil.example/", "data:text/plain,1", FALSE},
        {"https://evil.example/", "http://[::1/", FALSE},
        {"https:///", "http://127.0.0.1/", FALSE},
    };
    int failed = 0;
    for (gsize i = 0; i < G_N_ELEMENTS(cases); i++) {
        gboolean got = crosses_into_local(cases[i].page, cases[i].target);
        if (got != cases[i].blocked) {
            failed++;
            fprintf(stderr, "%s -> %s: %s, а должен быть %s\n",
                    cases[i].page ? cases[i].page : "(нет)", cases[i].target,
                    got ? "запрещён" : "разрешён", cases[i].blocked ? "запрещён" : "разрешён");
        }
    }
    if (failed) {
        fprintf(stderr, "правила локальной сети: ошибок %d\n", failed);
        return 1;
    }
    printf("правила локальной сети: %d случаев, всё верно\n", (int)G_N_ELEMENTS(cases));
    return 0;
}

#endif
