# Пакет для AUR: axiom-browser-bin

`axiom-browser-bin/` — то, что лежит в git-репозитории пакета в AUR: `PKGBUILD` и `.SRCINFO`. Пакет берёт готовый архив `axiom-linux-x86_64.tar.gz` из релиза `vX.Y.Z` на GitHub (тот же бинарник, что в `.deb`), сверяет SHA-256 и раскладывает файлы так же, как `packaging/arch/PKGBUILD`. Компилятор не нужен, зависимости (WebKitGTK 4.1, GTK 3, libsoup 3, xdg-utils, curl) ставятся через `pacman -S --needed --asdeps`, систему пакет не обновляет. `provides`/`conflicts` — `axiom-browser`: вместе с пакетом из репозитория `[axiom]` или сборкой из исходников он не ставится, pacman предложит удалить другой.

`install.sh` на Arch ставит именно этот пакет: через `yay` или `paru`, если пакет уже есть в AUR, иначе скачивает `PKGBUILD` из ветки `main` этого репозитория и запускает `makepkg -si`. Поэтому файлы здесь и в AUR должны совпадать.

CI в каждом запуске Release проверяет пакет в контейнере `archlinux:base-devel` (задача `aur`): `.SRCINFO` совпадает с `makepkg --printsrcinfo`, затем `bump.sh` подставляет версию и сумму только что собранного архива, `makepkg -si` собирает и ставит пакет от обычного пользователя, `namcap` выводит замечания. После выхода релиза Repositories ставит Axiom скриптом в чистом Arch обоими путями: по умолчанию (этот пакет) и с `AXIOM_ARCH_REPO=1`.

## Первая публикация (один раз)

1. Зарегистрируйтесь на https://aur.archlinux.org/register.
2. Сделайте SSH-ключ для AUR (в Git Bash на Windows тоже работает):
   ```sh
   ssh-keygen -t ed25519 -f ~/.ssh/aur -C aur
   ```
   Содержимое `~/.ssh/aur.pub` вставьте в поле «SSH Public Key» в «My Account» на aur.archlinux.org и сохраните. Пароль от аккаунта там же понадобится для подтверждения.
3. Добавьте в `~/.ssh/config`:
   ```
   Host aur.archlinux.org
     IdentityFile ~/.ssh/aur
     User aur
   ```
4. Клонируйте пустой репозиторий пакета (предупреждение «You appear to have cloned an empty repository» — это нормально: пакет появится после первого push):
   ```sh
   git clone ssh://aur@aur.archlinux.org/axiom-browser-bin.git
   cd axiom-browser-bin
   ```
5. Скопируйте `PKGBUILD` и `.SRCINFO` из `packaging/aur/axiom-browser-bin/` этого репозитория. На Arch можно пересобрать `.SRCINFO` самому: `makepkg --printsrcinfo > .SRCINFO`. На Windows копируйте готовый: CI проверяет, что он совпадает с выводом makepkg.
6. Отправьте. AUR принимает только ветку `master`, и в коммите должны быть оба файла:
   ```sh
   git add PKGBUILD .SRCINFO
   git commit -m "axiom-browser-bin 0.4.0"
   git branch -M master
   git push -u origin master
   ```
   Имя и почта из `git config` видны в истории пакета на AUR.
7. Проверьте страницу https://aur.archlinux.org/packages/axiom-browser-bin и установку: `yay -S axiom-browser-bin`.

## Каждый релиз

Сумма архива известна только после сборки, поэтому пакет обновляется после публикации релиза:

1. Дождитесь, пока Release выложит `vX.Y.Z` (в сводке задачи `aur` есть сумма архива).
2. Подставьте версию и SHA-256 из `SHA256SUMS.txt` релиза (Git Bash на Windows подходит):
   ```sh
   sh packaging/aur/bump.sh X.Y.Z
   ```
   Скрипт меняет `pkgver`, `pkgrel=1` и `sha256sums` в `PKGBUILD` и те же строки в `.SRCINFO`.
3. Закоммитьте `packaging/aur/axiom-browser-bin/` в `main`: оттуда `install.sh` берёт `PKGBUILD`.
4. Скопируйте оба файла в клон AUR и отправьте:
   ```sh
   git commit -am "axiom-browser-bin X.Y.Z"
   git push
   ```

Если меняется только упаковка (зависимости, список файлов), а версия та же, увеличьте `pkgrel` в `PKGBUILD` и `.SRCINFO` вручную. На Arch `.SRCINFO` после любой правки пересобирается командой `makepkg --printsrcinfo > .SRCINFO`.
