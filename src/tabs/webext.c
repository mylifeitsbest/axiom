/* Расширение веб-процесса WebKitGTK: подмена заголовка Referer во вкладках.
 *
 * Заголовки запросов WebKitGTK даёт менять только внутри веб-процесса — в сигнале
 * WebKitWebPage::send-request. build.rs собирает этот файл в .so, Axiom носит его
 * в себе и кладёт в профиль перед первой вкладкой (src/tabs/identity.rs).
 *
 * Выбор приходит от Axiom: при запуске процесса — в данных инициализации
 * "(usat)" (режим, свой адрес, id страниц вкладок), дальше — сообщениями всем
 * процессам: "axiom-referer" "(us)" и "axiom-tab" "(t)" (id страницы новой вкладки).
 * Страницы интерфейса и панели ИИ живут в тех же процессах — их не трогаем.
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

static gboolean on_send_request(WebKitWebPage *page, WebKitURIRequest *request,
                                WebKitURIResponse *redirect, gpointer data)
{
    (void)redirect;
    (void)data;
    if (mode == KEEP)
        return FALSE;
    guint64 id = webkit_web_page_get_id(page);
    if (!g_hash_table_contains(tabs, &id))
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

static void on_page_created(WebKitWebExtension *extension, WebKitWebPage *page, gpointer data)
{
    (void)extension;
    (void)data;
    g_signal_connect(page, "send-request", G_CALLBACK(on_send_request), NULL);
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
