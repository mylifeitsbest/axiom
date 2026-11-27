# Подпись exe через SignPath

Выпуски `Axiom-windows-x64.exe` подписывает бесплатная программа SignPath для открытых проектов: сертификат выдан SignPath Foundation, и в Windows издателем значится она. Сборка уже готова: `release.yml` отправляет exe на подпись, как только в репозитории есть переменная `SIGNPATH_ORGANIZATION_ID`. Пока её нет, exe выходит без подписи, как раньше.

Бета-версии не подписываются: SignPath требует вручную одобрять каждый запрос, а бета собирается из каждого коммита.

## Что сделать один раз

1. **Двухфакторная защита на GitHub** — у всех, кто может писать в репозиторий: GitHub → Settings → Password and authentication. Это условие SignPath.
2. **Заявка** на https://signpath.org/apply. Что указать:
   - проект: Axiom, https://github.com/mylifeitsbest/axiom, лицензия MIT;
   - что подписывать: `Axiom-windows-x64.exe` из выпусков, сборка — GitHub Actions (`.github/workflows/release.yml`);
   - Code signing policy: https://github.com/mylifeitsbest/axiom#code-signing-policy;
   - политика конфиденциальности: https://github.com/mylifeitsbest/axiom/blob/main/PRIVACY.md.
3. **Когда одобрят**, в SignPath:
   - в организации добавить trusted build system **GitHub.com** и подключить его к проекту;
   - проекту задать slug `axiom` и Repository URL `https://github.com/mylifeitsbest/axiom`;
   - Artifact configuration — содержимое [`artifact-configuration.xml`](artifact-configuration.xml);
   - signing policy со slug `release-signing` и сертификатом SignPath Foundation: включить **Verify origin** и **Use approval process**, одобряющий — ты. Если подпись выпуска по тегу (`refs/tags/v…`) откажут из-за ветки, проверь поле **Allowed branch names**;
   - пользователь для CI с ролью Submitter и его **API token**.
4. **В репозитории** (PowerShell, токен `gh` спросит сам и в историю он не попадёт):

```powershell
gh variable set SIGNPATH_ORGANIZATION_ID --repo mylifeitsbest/axiom --body "<Organization ID из SignPath>"
```

```powershell
gh secret set SIGNPATH_API_TOKEN --repo mylifeitsbest/axiom
```

   Если slug проекта или политики другие, задай их переменными `SIGNPATH_PROJECT_SLUG` и `SIGNPATH_POLICY_SLUG`.

## Проверка

Actions → Release → Run workflow с пустым тегом: сборка без выпуска. SignPath пришлёт письмо с запросом на подпись — одобри его. Задача `windows` проверит, что подпись действительна, сделана сертификатом SignPath Foundation и с отметкой времени, а в артефакте `windows` будет подписанный exe.

## При каждом выпуске

После тега `vX.Y.Z` SignPath пришлёт письмо — одобри запрос в течение часа, иначе задача `windows` упадёт по таймауту. Тогда перезапусти её: `gh run rerun <id> --failed`.
