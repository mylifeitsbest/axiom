# Встроенные списки фильтров рекламы

С ними Axiom блокирует рекламу сразу после установки и тогда, когда скачивание
свежих списков выключено («Настройки → Безопасность»). build.rs сжимает их в
программу (`src/adblock/lists.rs`); комментарии внутри списка выбрасываются,
заголовок с версией и лицензией остаётся.

- `easylist.txt` — [EasyList](https://easylist.to/), лицензия — GPLv3 или CC BY-SA 3.0
  ([подробно](https://easylist.to/pages/licence.html)).
- `ruadlist.txt` — [RuAdList](https://forums.lanik.us/viewforum.php?f=102)
  (`advblock.txt`), лицензия — CC BY 3.0.

Освежить перед выпуском:

```sh
curl -fsSL -o assets/adblock/easylist.txt https://easylist-downloads.adblockplus.org/easylist.txt
curl -fsSL -o assets/adblock/ruadlist.txt https://easylist-downloads.adblockplus.org/advblock.txt
```
