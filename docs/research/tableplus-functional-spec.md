# TablePlus (macOS) — functional feature checklist

Research date: 2026-10-12. TablePlus macOS was at **27.0.2 (build 818)** at that time
([changelog](https://tableplus.com/osx/changelog)).

**Method.** Built only from public web sources: every page of docs.tableplus.com (walked via
[llms.txt](https://docs.tableplus.com/llms.txt)), the tableplus.com homepage and blog, the macOS
changelog ([tableplus.com/osx/changelog](https://tableplus.com/osx/changelog)), and the public issue
tracker [github.com/TablePlus/TablePlus](https://github.com/TablePlus/TablePlus/issues), including
answers from the maintainer (`huyphams`) and screenshots he posted there. The TablePlus app was
**not** downloaded, installed or inspected.

**Scope.** PostgreSQL (PG), MySQL/MariaDB (MY) and SQLite (SL) only. Tags such as `(PG)` mark
behaviour that only applies to some drivers.

**How to read it**

- Each item is a `- [ ]` checkbox, so the file can double as Tabler's parity checklist.
- Each item cites its source:
  - `docs/...` → `https://docs.tableplus.com/...`
  - `#NNNN` → `https://github.com/TablePlus/TablePlus/issues/NNNN`
  - `bNNN` → the changelog entry for build NNN
- "(screenshot)" means the detail comes from a screenshot that the maintainer or a user posted in
  that issue. Screenshots are dated by the issue, and **labels may have shifted in later builds**.
  For example, "Favorites" became "Queries", the "Crash & Security" preferences tab became
  "Security", and a "Locations" preferences tab was added.
- Where sources disagree, both are listed and marked **⚠ conflict**.

---

## 0. Global map (menus, toolbar, window anatomy)

- [ ] Menu bar menus: **TablePlus, File, Edit, View, Connection, Plugins, Navigate, Window, Help** — #2491 (screenshot, 2021); the older order put Plugins before Navigate/View — #331 (screenshot). The native menu is on by default — [docs/menu](https://docs.tableplus.com/gui-tools/the-interface/menu)
- [ ] File menu items: Open…, Open Recent ▸, New Connection…, New Workspace ⌘N, New SQL Viewer ⇧⌘O, Reload Workspace ⌘R, Backup…, Restore…, Export…, Import ▸, Close — #331 (screenshot, 2018)
- [ ] Help menu: search field, Changelogs, **Enable SSH Debug log**, Private message…, Report bug & feedback — #2491 (screenshot); the debug-log item is also covered in [docs/troubleshooting](https://docs.tableplus.com/utilities/troubleshooting) and [blog: failed SSH](https://tableplus.com/blog/2018/10/failed-to-connect-via-ssh-tableplus.html), which calls it "Enable SSH & Bash Script Log"
- [ ] Connection menu: View Using Encoding…, User Management…, Process List…, Reconnect, Disconnect — [docs/working-with-table/table](https://docs.tableplus.com/gui-tools/working-with-table/table), [docs/user-management](https://docs.tableplus.com/gui-tools/user-management), [docs/query-editor/untitled](https://docs.tableplus.com/query-editor/untitled), #2756, b160 (adds Disconnect). A later issue calls it "Tool(s) → Process List…" — #3289
- [ ] Window menu: Move Tab to new Window — [docs/multi-tabs](https://docs.tableplus.com/gui-tools/the-interface/multi-tabs-workspaces-windows). Main menu has "Copy Cell Value" — b700. "Close all Tabs" — b354
- [ ] Toolbar, left → right:
  - Action control (Discard / Preview / Commit)
  - Safe-mode lock button
  - Database list and SQL buttons
  - Status bar (click it to see details and recolour the connection)
  - Refresh
  - Open Anything (search)
  - Show Workspaces
  - User Management
  - Metrics Board
  - Toggle Panels (left sidebar / console / right sidebar)

  Sources: [docs/toolbar](https://docs.tableplus.com/gui-tools/the-interface/toolbar), [docs/metrics-board](https://docs.tableplus.com/gui-tools/metrics-board)

- [ ] The status bar text format is `TAG | Driver version : [SSH/TLS :] connection name : database : schema.table` — #3881 (screenshot), #1205, #1702
- [ ] Right-click the toolbar → **Customize Toolbar** (Safari-style). Available items:
  - Action control
  - Safe Mode
  - Open Anything
  - Status
  - Toggle Panels
  - Show Workspaces
  - Reload Workspace
  - User Management
  - space / flexible space
  - "default set"

  Sources: [docs/toolbar](https://docs.tableplus.com/gui-tools/the-interface/toolbar), [blog: customize](https://tableplus.com/blog/2018/10/customize-tableplus-experiences-mac.html)

- [ ] Every button has a tooltip that shows its shortcut — [docs/the-interface](https://docs.tableplus.com/gui-tools/the-interface), #3337
- [ ] Free-tier limits: max **2 open tabs, 2 windows, 2 advanced filters** at a time — [docs/licensing](https://docs.tableplus.com/utilities/licensing)

---

## 1. Connections

### 1.1 Welcome screen / connection list

- [ ] The welcome screen lists saved connections with groups. Double-click a connection to open it. The **Create a new connection** button is at the bottom; right-click empty space → **New ▸ Connection… / Group…** — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), [docs/getting-started](https://docs.tableplus.com/getting-started), [blog: getting started](https://tableplus.com/blog/2018/04/getting-started-with-tableplus.html)
- [ ] New connection flow: choose the driver → **Create** → driver form. The new-connection sheet also has **Import from URL** — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections)
- [ ] Connection context-menu items:
  - **Edit…**
  - **Copy as URL** (⌘C on a selected connection also copies the URL, _including the password_). If the connection has safe mode, Touch ID is required before copying.
  - **Open in new window**
  - Duplicate/clone ("clone the connection … also clones that setting")
  - Delete
  - Export Connections ▸
  - Import Connections…

  Sources: [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), #1689, #2398, #1671, [docs/multi-tabs](https://docs.tableplus.com/gui-tools/the-interface/multi-tabs-workspaces-windows)

- [ ] Search connections with ⌘F on the home screen — b348, b200, b352
- [ ] Sort connections by name / tag / driver — b246
- [ ] **Backup** and **Restore** buttons on the welcome screen — [docs/backup-and-restore](https://docs.tableplus.com/gui-tools/backup-and-restore)
- [ ] "Default connection" preference (older builds): show the welcome screen, or auto-open a chosen connection at startup, optionally prompting for the input form — [blog: customize](https://tableplus.com/blog/2018/10/customize-tableplus-experiences-mac.html), #1475 (screenshot of the "Behaviours" prefs tab)
- [ ] Connections are saved when you press **Connect** or **Test**, not only on **Save** — #2899, #3062, b322

### 1.2 Connection form — fields common to every driver

Layout as of PG 2022, MySQL 2020 and later screenshots (#2531, #2716, #533, #2400):

- [ ] **Name**
- [ ] **Status color**: the selected swatch plus 5 preset swatches. An older form also had a free colour picker and a **Hex** field — #55 (screenshot), [blog: customize-colors](https://tableplus.com/blog/2019/10/customize-colors.html)
- [ ] **Tag** dropdown. Built-in tags: **local, testing, development, staging, production**. Custom tags with custom colours have been allowed since b494 — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), b494, #1732, #2967
- [ ] **Host** (MY) / **Host/Socket** (PG) and **Port**. Defaults are 3306 / 5432 — #2716, #533
- [ ] **User**
- [ ] **Password** plus a storage mode: **Store in keychain** / **Ask every time** / **No password** / **Command Line (IAM authentication)** — #2531 (screenshot)
  - "Ask every time" still keeps the password in memory for the session so auto-reconnect can use it — #2068
- [ ] **Database**
- [ ] **Bootstrap commands…** button (see §1.6)
- [ ] **SSL mode** dropdown and **SSL keys**: **Key…**, **Cert…**, **CA Cert…** buttons plus a "—" button that clears them — #2531, #2716, #3562 (screenshots)
- [ ] **Other options** dropdown with driver-specific toggles — #2716, #533
- [ ] **Ver.** (driver version) dropdown for MySQL — #2400, #2804
- [ ] Bottom buttons: **Over SSH** (toggles the SSH section, with a version dropdown next to it), **Save**, **Test**, **Connect** — #3562 (screenshot)
- [ ] Credentials go to the macOS Keychain (service per connection, keyed by UUID). Nothing is synced to TablePlus servers — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), #1689, #193
- [ ] Connection data lives in `~/Library/Application Support/com.tinyapp.TablePlus/`. The location is configurable in Preferences → **Locations**. Changing it does _not_ move existing data, so export and import connections instead — #193, #83, #2783, b296

### 1.3 PostgreSQL form specifics

- [ ] SSL mode values seen: **PREFERRED** (default) and **VERIFY-FULL**; DISABLED also appears. The full list was not visible in any public screenshot; it is presumably the libpq set (disable/allow/prefer/require/verify-ca/verify-full) — #2716, #2531, #3842
- [ ] **SSL negotiation mode**: POSTGRES / DIRECT (b676, 2026). Choosing DIRECT with SSL mode DISABLED gives a libpq error — #3842, #3827, b676
- [ ] Option to **disable channel binding** — b574
- [ ] SNI support — b436
- [ ] Extra libpq parameters can be passed through the connection URL (e.g. `jit=off`) — #3422
- [ ] **Command Line (IAM authentication)** password mode: paste a shell command (e.g. `aws rds generate-db-auth-token …`) whose output becomes the password. Login-shell env is loaded (b468) — #2531, #3379, b454
- [ ] **Lazy load schema items** per connection (URL param `lazyload`). It is a sidebar setting, on by default in some builds and off in others (b402, b546, b608) — #3580, #3109

### 1.4 MySQL / MariaDB form specifics

- [ ] **Ver.** dropdown:
  - older builds: MySQL 8.x / MySQL 5.x — #2400, #533
  - later builds: **Default** / **Old – Old MySQL** / **SingleStore DB** — #2804 (screenshots); the old driver supports TLS 1.1 — #2539
  - b113 added dual drivers for MySQL 8 and 5
- [ ] **Use socket** checkbox with a socket path field (placeholder `/tmp/mysql.sock`). The socket is auto-detected (b108). Socket over SSH is supported (b228) — #2400, #533, #55
- [ ] SSL mode values:
  - default MySQL driver: **PREFERRED, DISABLED, REQUIRED, VERIFY-CA, VERIFY-IDENTITY** — #1236 (screenshot)
  - MariaDB / SingleStore / old driver: **PREFERRED, ENFORCE, VERIFY-SERVER-CERT** — #2804 (screenshot), #1774, #1495
- [ ] TLS 1.3 support (b516, b532); SNI (b574)
- [ ] **Other options**: **Enable CLEARTEXT_PLUGIN**, **Enable LOAD DATA LOCAL INFILE** — #533 (screenshot), #384
- [ ] Connection encoding can be changed (b170)
- [ ] Long queries over SSH: test builds in #1932 tried `max_execution_time`, `MYSQL_OPT_READ_TIMEOUT=0` and SSH keep-alive / ClientAliveInterval. The root cause was the SSH timeout — #1932, #1356

### 1.5 SQLite form specifics

- [ ] Fields: database file path, plus "**create a new SQLite DB**" by choosing a folder — #947, [docs/getting-started](https://docs.tableplus.com/getting-started)
- [ ] Double-click a `.db` / `.sqlite` / `.sqlite3` file to open it in TablePlus — #444, [docs/getting-started](https://docs.tableplus.com/getting-started)
- [ ] **Encrypted key** (SQLCipher). Encrypted DBs open on double-click (b336) — b100, #347
- [ ] SQLite **over SSH** (b233): the file is pulled from the server, so it only suits small DBs — #1339
- [ ] Bootstrap commands (e.g. `PRAGMA foreign_keys = ON;`). An older form had a "resource file" (`.sqliterc`) option — #3549, #393
- [ ] The SQLite file is closed when the app closes (b364). WAL/SHM leftovers were reported — #2299

### 1.6 Bootstrap commands / startup scripts

- [ ] **Bootstrap commands…** opens an SQL editor. Its contents run right **after login** on every connect, e.g. `SET SQL_SAFE_UPDATES=1;`, `SET search_path…`, `PRAGMA …` — #2400 (screenshot), #624, #2494, b308
- [ ] Bootstrap **CLI/bash script** that runs before connecting (e.g. to copy or rsync a SQLite file, or start a proxy). Added in b546 ("Bootstrap CLI script when creating a new connection") — #2531, #3732, #1709
- [ ] Bootstrap SQL can also be appended through a URL `raw=` filter hack when opening from the CLI — #3549

### 1.7 SSH tunnel (Over SSH)

- [ ] Fields — #815 (screenshot), #55:
  - **Server**, **Port** (22)
  - **User** (placeholder: leave empty if you use `ssh/config`)
  - **Password** plus a storage mode
  - **Use SSH key** checkbox with **Import a private key…** and a "—" (remove) button
  - hint: if the private key is left empty, `~/.ssh/config` is used
- [ ] `~/.ssh/config` support needs **Use SSH key** to be ticked: the Host alias goes in Server, the key is left empty. ProxyCommand/ProxyJump and IdentityAgent (e.g. the 1Password agent) work through the config — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), [blog: failed SSH](https://tableplus.com/blog/2018/10/failed-to-connect-via-ssh-tableplus.html), #166, #586, #3814
  - Since 6.8.6, form values (user/port) take priority over the config — #3814, b662
- [ ] **SSH version** dropdown next to Over SSH: "SSH Version 0.11.1 / 0.9.5" (#3562 screenshot); b730 added 0.12 — #3933, b730
- [ ] Auth methods: password, private key with passphrase (stored in the Keychain), **keyboard-interactive / 2FA prompts** (questions are forwarded to the user), ssh-agent through the config. Key types: RSA, ECDSA, ED25519, OpenSSH format — [blog: keyboard-interactive](https://tableplus.com/blog/2018/05/tableplus-supports-keyboard-interactive-authentication-for-ssh.html), #1682, #2522, #664, b150
- [ ] Built-in libssh tunnel (no external `ssh` process). The local port only accepts connections from TablePlus — [blog: ssh leak](https://tableplus.com/blog/2017/06/how-using-ssh-tunneling-in-database-tools-might-leak-your-connection-forever.html)
- [ ] Known-hosts validation through `~/.ssh/known_hosts` — #1230
- [ ] Debug log via Help → Enable SSH Debug Log — [docs/troubleshooting](https://docs.tableplus.com/utilities/troubleshooting)

### 1.8 Groups, tags, colours

- [ ] Groups: right-click → **New group…** (name + icon/image). Drag connections in and out. Right-click a group → **Edit…** / **Delete**. Nested groups since b296 — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), b296
- [ ] Recolour the connection while connected by clicking the status bar (preset or custom colour) — [docs/toolbar](https://docs.tableplus.com/gui-tools/the-interface/toolbar), [blog: customize-colors](https://tableplus.com/blog/2019/10/customize-colors.html)
- [ ] The sidebar database icon takes the status-bar colour — b154

### 1.9 Import / export connections

- [ ] Right-click a group → **Export Connections ▸**: **Export all…** / **Export this group…** / **Export this connection…**. You choose whether to include database passwords and server (SSH) passwords, set a file password, then **Export**. Output is a `.tableplusconnection` file that includes group names and images — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), b115, b112, b125
- [ ] Right-click the welcome screen → **Import Connections…** → pick the file → enter its password → **Import** — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections)
- [ ] Safe-mode level is part of the exported data (a regression fixed in 2025) — #3553, b604
- [ ] No cloud sync. The "Locations" folder can be put on Dropbox or iCloud, but passwords stay in the Keychain — #193, #3034, b296

### 1.10 Connect via URL / CLI / deep links

- [ ] **Import from URL** in the new-connection sheet, e.g. `postgres://user:password@host:port/database` — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections)
- [ ] Open from Terminal: `open -a TablePlus "<url>"`. All values must be URL-encoded — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections)
- [ ] Supported query parameters — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), #1415, #914, #845:
  - `statusColor` (hex without `#`), `env` (tag; older form `enviroment`), `name`
  - `windowMode` = `tabbed` | `isolated`
  - `tLSMode` (int), `usePrivateKey` (bool)
  - `safeModeLevel` (int), `advancedSafeModeLevel` (1 = ignore SELECT/EXPLAIN)
  - `driverVersion` (int), `lazyload` (bool)
  - `schema`, `table` or `view`
  - filter: `column`, `operation`/`operator`, `value`, **or** `condition`/`raw`/`query` (raw WHERE)
- [ ] SSH in URLs: `postgresql+ssh://sshuser:sshpass@sshhost/dbuser:dbpass@127.0.0.1/db?...` — #845
- [ ] Row-level deep link (b570): open a table pre-filtered by column/operator/value or by a raw condition — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), #1593
- [ ] Open a saved connection by its (undocumented) connection ID — b121, #1593

### 1.11 Session behaviour

- [ ] **Keep connection alive**: pings every **30 s**, on by default (pref) — [docs/preferences/connections](https://docs.tableplus.com/preferences/general/connections), [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections)
- [ ] **Query time out**: default **300 s**. TablePlus auto-kills queries with no response after that — [docs/preferences/connections](https://docs.tableplus.com/preferences/general/connections), #1745 (screenshot)
- [ ] Connect timeout is 60 s (2018 patch). If the server is unresponsive the app asks whether to reconnect — #216, b136
- [ ] **Force reconnect ⌘⇧R** (also Connection → Reconnect, or click the header on disconnect). Auto-reconnect when the app becomes active (b308). Pending changes are kept across reconnect (b308). The workspace is not closed on disconnect (b107) — #2756, b136, b308, b107
- [ ] The SQL Query disconnect popup has a **Reconnect** button — b568

### 1.12 Safe mode levels (exact names) and read-only

The lock icon in the toolbar opens a menu. Exact labels from 2025 (#3881 screenshot):

- [ ] **Silent Mode** — send queries to the server without any warnings
- [ ] **Alert Mode 1** — warn before sending queries to the server
- [ ] **Alert Mode 2** — warn before sending queries to the server **except SELECT/EXPLAIN/SHOW** queries (docs say SELECT/EXPLAIN; SHOW was added later)
- [ ] **Safe Mode 1** — prompt for password before sending queries to the server
- [ ] **Safe Mode 2** — prompt for password before sending queries to the server except SELECT/EXPLAIN/SHOW queries

Sources for the list: [docs/safe-mode](https://docs.tableplus.com/gui-tools/code-review-and-safemode/safe-mode), [docs/toolbar](https://docs.tableplus.com/gui-tools/the-interface/toolbar), #3881, b222

- [ ] Changing the level may itself ask for the DB password. The level is stored **per connection**, and Preferences → Security has "Default Safe Mode for new connection" — [docs/safe-mode](https://docs.tableplus.com/gui-tools/code-review-and-safemode/safe-mode), #2669, #3312, #3596 (screenshot), b396
- [ ] **Unlock Safe Mode with Touch ID** / computer password (pref) — [docs/crash-and-security](https://docs.tableplus.com/preferences/crash-and-security), b170, [blog: build 270](https://tableplus.com/blog/2019/10/whats-new-in-build-270.html)
- [ ] Safe mode also covers:
  - comments in queries are ignored when classifying (b260)
  - CTE queries (b546; the changelog says "CTA")
  - backup/restore (b330)
  - user management and the import wizard (b194)
  - killing queries (#783)
  - viewing a connection's URL/password (#2398)
  - connections saved without a password then prompt for one (#3881)
- [ ] **No separate read-only mode.** The maintainer points to the safe-mode levels instead and recommends a read-only DB user — #539, #2912, #109, #1671. A SQLite read-only FR is still open — #2629

---

## 2. Workspace, tabs and left sidebar

### 2.1 Workspaces, windows, tabs

- [ ] Switching to another DB or connection opens a new **workspace tabbed in the same window** by default:
  - ⌘K → double-click a DB opens it in a new workspace; right-click → **Open in current workspace** replaces the current DB
  - ⌘⇧K → double-click a connection; right-click → **Open in new window**
  - ⌘N opens a new window
  - Window → **Move Tab to new Window**

  Source: [docs/multi-tabs](https://docs.tableplus.com/gui-tools/the-interface/multi-tabs-workspaces-windows)

- [ ] **Show Workspaces** (toolbar, ⌘⇧\\) shows all open workspaces so you can switch or add one — [docs/toolbar](https://docs.tableplus.com/gui-tools/the-interface/toolbar), #150
- [ ] **Preview vs pinned item tabs** — [docs/multi-tabs](https://docs.tableplus.com/gui-tools/the-interface/multi-tabs-workspaces-windows), #3795, #2510, #2454:
  - Opening an object replaces the current tab if that tab is unmodified (a preview tab, shown in italics).
  - A modified tab is pinned automatically.
  - Double-click an item to open it pinned.
  - Right-click → **Open in new tab** always opens a new tab.
  - Holding ⌘ while opening from Open Anything opens a new tab.
- [ ] Tab context menu: **Rename Tab**, **Close tab**, **Close other tabs**, **Close tabs to the right**. "Close all Tabs" is also in the main menu (b354) — [docs/multi-tabs](https://docs.tableplus.com/gui-tools/the-interface/multi-tabs-workspaces-windows)
- [ ] ⌘T opens a new tab. Since b388 / 2021 it opens an **SQL Query** tab; the pref "Command + T should open" offers SQL Query Editor tab / empty tab — #3778, #1427, #3583 (screenshot)
- [ ] ⌘W closes the tab. ⌘1…9 jumps to a tab. ⌘[ / ⌘] move to the previous/next tab — [docs/shortcut-keys](https://docs.tableplus.com/utilities/shortcut-keys)
- [ ] Back/forward navigation (b586). Split the **workspace** horizontally (b586) and split the screen to the right with ⌘⇧D (b600) — b586, b600
- [ ] One table can be open in several tabs (b182). Item tabs are not closed when the schema changes (b546)
- [ ] Restore session: "**Reopen closed workspaces at startup**" (pref). Auto-restore of recently closed workspaces has been on by default since b556 — [docs/manage-connections](https://docs.tableplus.com/gui-tools/manage-connections), [docs/application](https://docs.tableplus.com/preferences/general/application), b240, b556
- [ ] SQL tabs restore on reopen per the "Auto Save SQL Query Tab" setting (§5.1); b632 added an option to restore all SQL Query tabs — #3626 (screenshot), b632

### 2.2 Left sidebar — top bar and tabs

- [ ] Quick navigation buttons: **Show Connections ⌘⇧K**, **Show Databases ⌘K**, **Open Query Editor ⌘↩** — [docs/left-sidebar](https://docs.tableplus.com/gui-tools/the-interface/left-sidebar)
- [ ] Tabs: **Items | Queries (formerly Favorites) | History** — #1926, #1702 (screenshots), [blog: keyword binding](https://tableplus.com/blog/2018/08/keyword-binding-favorite.html)
- [ ] When several databases are open, a second narrow sidebar of DB icons appears for quick switching. A settings button toggles **Show/hide database name** and **Show/hide connection name** — [docs/left-sidebar](https://docs.tableplus.com/gui-tools/the-interface/left-sidebar), #917
- [ ] Reorder databases in that bar (b490). Toggle the sidebar with ⌘0 — #3337, #3802

### 2.3 Items list contents

- [ ] Sections:
  - **Recent** — the last 5 opened items, optional — [docs/left-sidebar](https://docs.tableplus.com/gui-tools/the-interface/left-sidebar)
  - **Functions** — functions and procedures, optional
  - **Tables** — tables, **views** (eye icon), materialized views
- [ ] (PG) **Sequences** (b478), shown via sidebar settings "show sequences" — #2066
- [ ] Per-table **column list** in the sidebar (b670), off by default — #3094, #3491
- [ ] Enums and types are _not_ listed as a separate section (FR still open) — #2215
- [ ] Triggers are shown inside the table structure, not in the sidebar — [docs/trigger](https://docs.tableplus.com/gui-tools/working-with-table/trigger)
- [ ] **Groups with filter rules** (b446): New ▸ Group… → name + "**Use filter rules**" (e.g. has prefix, is view). Used to group partitions or separate views — #935, #2165, #3463, b446
- [ ] **Pin to top** / **Unpin from top** (respects multi-selection; pinned items leave the alphabetical list) — [docs/left-sidebar](https://docs.tableplus.com/gui-tools/the-interface/left-sidebar), #3754, b270
- [ ] **Schema switcher** dropdown at the bottom (PG). Show/hide system schemas (information_schema, pg_catalog, pg_toast) — [docs/left-sidebar](https://docs.tableplus.com/gui-tools/the-interface/left-sidebar), #2926
- [ ] Bottom **+** button with a menu — #2815 (screenshot, 2022), [docs/getting-started](https://docs.tableplus.com/getting-started):
  - New Table / New View / New Materialized View / New Function/Procedure / New Schema… / New Group…
  - **Settings ▸**: Show recently section, Show function section, Show system schemas, Lazy load schema items
- [ ] Search field: fuzzy match — [docs/left-sidebar](https://docs.tableplus.com/gui-tools/the-interface/left-sidebar), b118, b222, b286, b490, #3580 (screenshot), #1286
  - regex style `^name$`; `|` or `,` for OR; AND regex
  - double quotes for an exact pattern
  - ⌘⇧F focuses the field
  - its menu has **Search in Database…**, **Matching strategy ▸**, **Settings ▸** (Show recently section, Load system schemas, Lazy load item in schema)
- [ ] Header "Tables" click / right-click → **Overview items** opens the Item Overview grid (§11.2) — [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table)

### 2.4 Sidebar item context menu (table / view)

Order from #1464 and #1702 (screenshots, 2019–2020):

- [ ] **Open in new tab**
- [ ] **Open structure**
- [ ] **Item overview** (older label "Overview items")
- [ ] **Copy name** (applies to every selected item)
- [ ] **Pin to top**
- [ ] **Export…** (all selected items)
- [ ] **Import ▸** with **From CSV…** and **From SQL Dump…** — [docs/import-and-export](https://docs.tableplus.com/gui-tools/import-and-export)
- [ ] **New ▸**
- [ ] **Clone…** — duplicates the table (structure + data, Sequel-Pro style). It drops FKs and cannot choose a target schema — #2388, #1680, #2519, #3573
- [ ] **Truncate…** ⌥⌫
- [ ] **Delete…** ⌘⌫ (the docs also allow just pressing Delete) — [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table)
- [ ] Delete/truncate modifiers — #155, #1459, #1216, b233, b270, #3216, #3242:
  - plain Delete = drop
  - ⌥⌫ = truncate
  - ⇧⌫ = delete **CASCADE** (PG)
  - ⌘⌫ = delete **ignoring foreign-key checks** (MY)
  - The right-click Truncate dialog offers "disable foreign key check" and Cascade. The shortcut skips the dialog.
  - One statement is generated per table.
- [ ] Truncate and Delete are **staged** and need ⌘S to commit — [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table)
- [ ] **Copy Script As ▸ CREATION / DROP / TRUNCATE / Laravel Migration**. Originally the Dump Table plugin, later "copy as creation". Only the clicked table is used — [docs/plugin](https://docs.tableplus.com/utilities/plugin), #3619, #3495, #3467
- [ ] **Generate diagram** (Diagram plugin, "create diagram from selected items"). A native diagram was added in b718 — #636, #2012, b718
- [ ] Functions/procedures: open, **Delete** (or the Delete key) — [docs/function](https://docs.tableplus.com/gui-tools/database-objects/function), [docs/procedure](https://docs.tableplus.com/gui-tools/database-objects/procedure)

---

## 3. Data grid (table data view)

### 3.1 Loading, paging, counting

- [ ] Default **300 rows per page**. The bottom-bar settings/paging control has **Limit**, **Offset** and **Go**. ◀ ▶ buttons page through, and the view scrolls to the top on page change (b256) — [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table), #3183, #1702 (screenshot)
- [ ] Generated query: `SELECT * FROM t ORDER BY … LIMIT 300 OFFSET 0`. Sorting is server-side — #1019, #3592
- [ ] Footer counter reads "1-237 of 237 rows" / "15 of ~999,985 rows selected". `~` marks an estimate — #1702, #420 (screenshots), b136
  - Pref "Estimate count when table has more than **500000** rows" (on) — #1745
- [ ] Bottom bar: **Data | Structure** toggle (⌘⌃[ / ⌘⌃]), **+ Row**, the counter, **Columns**, **Filters**, page arrows, ⚙ — #1702, [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table)
- [ ] Table data/structure only loads when needed (b128). Large-grid performance work is ongoing (b650)

### 3.2 Sorting and columns

- [ ] Click a column header: sort descending → ascending → off (per docs) — [docs/row](https://docs.tableplus.com/gui-tools/working-with-table/row)
- [ ] **Multi-column sort** (b204) from the header context menu (b388)
- [ ] "Default content order" option in table view settings (b632)
- [ ] Header context menu: **Copy name**, **Filter with column**, **Hide this column**, **Reset column Positions and Widths**, **ASC**, **DESC**, **Remove sort** — #1702, #1371 (screenshots), b388
- [ ] Column state (position, width, hidden) is remembered per table (b204, b296). Double-click a header edge to auto-fit — #2728
- [ ] **Column filter** (⌘⌥F or the **Columns** button): an "Add a column" picker, then **Clear** and **Apply** — [docs/filter](https://docs.tableplus.com/gui-tools/filter), #1702 (screenshot)
- [ ] Toggle row numbers on table views and results (b432)

### 3.3 Selection, copy, paste

- [ ] Row selection with ⌘A. Cell-level selection (b314); a "**Copy Selected Cells (As)**" red-border selection was added in b762 — #3914, #3773
- [ ] Cell context menu, 2018 order — #797 (screenshot):
  - Add Row ⌘I
  - Quick Look (middle mouse)
  - Refresh
  - Paste ⌘V
  - Duplicate ⌘D
  - Copy ⌘C, Copy Cell, Copy Columns, Copy Rows As ▸
  - Quick Filter ▸
  - Import ▸, Export current page…
  - Delete ⌫
- [ ] Cell context menu, 2021 order — #2459 (screenshot), [docs/quick-look](https://docs.tableplus.com/gui-tools/the-interface/quick-look):
  - **Quick Look Editor ⌘↩**
  - **Refresh ⌥⌘R**
  - Paste ⌘V, Add Row ⌘I, Duplicate ⌘D
  - **Set Value ⌥↩ ▸**
  - Copy ⌘C, **Copy Cell Value**, **Copy All Column Values**, Copy Rows As ▸
  - Quick Filter ▸
- [ ] **Copy Rows As ▸**: Plain text, JSON, HTML, Markdown table, CSV, CSV with header ("CSV include fields name"), SQL Insert statement, plus "copy as Raw" (b160) — [docs/row](https://docs.tableplus.com/gui-tools/working-with-table/row), #3915, b160, b200
  - It respects the visible/selected columns (b174) and the current column order — #866, #1010
  - The CSV delimiter follows Preferences → CSV default options (so it can produce TSV) — #3651
  - Insert statements are split into ~250 KB chunks — #1645
- [ ] "Copy Columns" / "Copy All Column Values" copies one column as a comma-separated list, single-quoted for strings (b314). Later behaviour changed to newline-separated — #350, #951, #3923
- [ ] ⌘C copies rows in an internal CSV-like format that keeps NULL vs EMPTY. ⌘V pastes them as **new rows** and keeps PK values. ⌘D duplicates and clears auto-increment PKs — #1043, #1355, #1491, [docs/row](https://docs.tableplus.com/gui-tools/working-with-table/row)
- [ ] ⌘⇧C copies the cell value (b446) **⚠ conflict**: the docs list ⌘⇧C as "Toggle console log"
- [ ] Paste into existing rows/columns is not supported (FR) — #3857

### 3.4 Editing cells and rows

- [ ] Double-click a cell (or Enter) to edit inline. Tab moves to the next cell while editing. Changes are **staged** (modified cells tangerine/yellow, new rows green, deleted rows red) until ⌘S — [docs/row](https://docs.tableplus.com/gui-tools/working-with-table/row), [blog: customize-colors](https://tableplus.com/blog/2019/10/customize-colors.html), [docs/shortcut-keys](https://docs.tableplus.com/utilities/shortcut-keys)
- [ ] Insert a row with **+ Row**, ⌘I, or double-click the blank area. Delete with the Delete key or right-click **Delete** — [docs/row](https://docs.tableplus.com/gui-tools/working-with-table/row), #1923
- [ ] **Set Value ⌥↩ ▸** — #2459 (screenshot), #1951, #2205, #100, b360:
  - **EMPTY**, **NULL**, **DEFAULT**, **Add a file…** (load a file into a blob)
  - plus NOW() for date types — #71, #2337
  - ⌥+click a cell opens the same quick-edit menu
- [ ] Pref "**Auto convert empty Numeric, UUID, JSON or Array to NULL**" (on). The literal string "NULL" is _not_ converted — #1475 (screenshot), #1131, #206
- [ ] Typing NOW() in a date cell is accepted (2019 patch). Date cells have a date picker — #1403, #71
- [ ] Boolean / enum / timestamp cells show a stepper or dropdown (⌃⇣ icon in cell) — #1702, #2459
- [ ] Undo ⌘Z / Redo ⌘⇧Z inside staged edits — [docs/shortcut-keys](https://docs.tableplus.com/utilities/shortcut-keys)
- [ ] Bytea edits as hex (b222). MySQL binary shows and edits hex (b270). Binary UUID PKs and binary FKs are supported (b150) — b222, b270, b150
- [ ] Tables without a PK: updates compare **all columns** with `LIMIT 1`, which fails on floats. A PK is recommended — #3389, #3400

### 3.5 Right sidebar (row detail)

- [ ] Toggle with **Space** or the top-right button. It shows every field of the selected row as an editable form, with column type labels and a **column search** field — [docs/right-sidebar](https://docs.tableplus.com/gui-tools/the-interface/right-sidebar), [docs/getting-started](https://docs.tableplus.com/getting-started)
- [ ] **Bulk edit**: with several rows selected, fields show "Multiple values". Editing one sets that field for all selected rows. Each field's dropdown offers NULL / DEFAULT / NOW() — #2337 (screenshot), #1491
- [ ] Shows the raw value. A pretty JSON option sits in the field dropdown (b190) and changes the JSON. Multiple carets and syntax highlighting in fields (b300) — #2752, b190, b300
- [ ] Since b624 the right sidebar also hosts the LLM chat / Assistant tab (§11.6)

### 3.6 Viewers: Quick Look, JSON, binary, geometry

- [ ] **Quick Look Editor**: right-click → Quick Look, middle-click, or ⌘↩. A bigger popup editor that can be detached and float (b512) — [docs/quick-look](https://docs.tableplus.com/gui-tools/the-interface/quick-look), b512, #2250, #3223
  - JSON/XML syntax highlighting (b300), search and replace (b300), multiple carets (b276)
  - a **Show raw** button (b314)
- [ ] JSON viewer with sorted keys (b117, b210), **tree/viewer mode** (#2530, #3223), Json and Tree views (b818). Auto-pretty JSON (b710)
- [ ] Blob: decoded to text when possible (b272). PG bytea attachments (e.g. images) show in Quick Look — #427
- [ ] Quick Look also has a **foreign-key viewer** (b320): ⌘↩ on an FK cell shows the referenced row(s) — #3782, #1678
- [ ] Geometry: a viewer on hover (b290), spatial data in a **Map view** (b308), 3D/4D geometry (b336). Pref "**Automatically convert PostGIS geometry to WKT**" (on) — #1676
- [ ] The macOS tooltip shows a long value on hover; pref "**Allows expansion Tool Tips**" — #1529, #1745

### 3.7 Foreign-key navigation

- [ ] FK cells show an → arrow. Clicking it jumps to the referenced table, filtered to that value. If a column has several FKs, a menu appears. ⌥+click, ⌘↩ and ⌥↩ are keyboard alternatives — #516, #1156, [docs/filter](https://docs.tableplus.com/gui-tools/filter)
- [ ] (MY) Cross-database FKs can be opened (b286)
- [ ] Auto-generate JOIN queries from FKs (b422)

### 3.8 Row filter (advanced filters)

- [ ] Open with ⌘F or the **Filters** button. Each filter row has a checkbox, a column dropdown, an operator dropdown, a value field, an **Apply**/**Applied** button, **−** and **+**. Under the rows: ⚙, **Clear All** (later "**Unset**"), **SQL** (shows the generated query), **Export** (b360), and a shortcut hint line — [docs/filter](https://docs.tableplus.com/gui-tools/filter), #635, #1612, #2121, #1702 (screenshots)
- [ ] Column dropdown: **Raw SQL** (free WHERE text with autocomplete), **Any column**, then every table column — #635, #722, #1456
- [ ] Operator list, PostgreSQL driver — #2731 (screenshot, 2022):
  - `=`, `<>`, `<`, `>`, `<=`, `>=`
  - `IN`, `NOT IN`
  - `IS NULL`, `IS NOT NULL`
  - `BETWEEN`, `NOT BETWEEN`
  - `Contains`, `Not contains`
  - `Contains - Case insensitive`, `Not contains - Case insensitive`
  - `Has prefix`, `Has suffix`
  - `Has prefix - Case insensitive`, `Has suffix - Case insensitive`
- [ ] Operator list, MySQL driver — #2948 (screenshot, 2023): the same list, but the variants read **"- Case sensitive"** (Contains - Case sensitive, Not contains - Case sensitive, Has prefix - Case sensitive, Has suffix - Case sensitive)
- [ ] Later additions:
  - `LIKE` (2023) — #2948
  - `ILIKE` (b222) — #440
  - (PG) JSON `contains` / `not contains` (b432) — #2731
  - MySQL case-sensitive filter (b282)
  - IN/NOT IN auto-add braces (b190) — b200
- [ ] Apply one filter with Return or its Apply button — [docs/filter](https://docs.tableplus.com/gui-tools/filter), #1577 (screenshot), b300
- [ ] **Apply All** (⌘↩) applies the checked filters combined with **AND**. The Apply All dropdown has "**Apply All Checked Filters with AND – Default**" (⌘↩) and "**… with OR**" (⇧⌘↩)
- [ ] Keyboard: ⌘I duplicate/insert a filter (clones column + operator), ⌘⇧I remove, ⌘↑/↓ move between filters, ⌘B toggle the checkbox, ⌘←/→ open the column/operator popups, Esc closes — [docs/filter](https://docs.tableplus.com/gui-tools/filter), #793, #1612, #1041, #2238
- [ ] ⚙ settings — [docs/filter](https://docs.tableplus.com/gui-tools/filter), #635, #457 (screenshots), b260:
  - **Default filter column**: Primary key (if exist) / Any column / Raw SQL
  - **Default filter operator**: `=` / `Contains`
  - **Default filter state**: Restore the last filter state / Always show / Always hide
- [ ] Filters are remembered per table, including history and show/hide state (b204, b228). ⌘F focuses the last applied filter (b190) — #650
- [ ] Quick filters — [docs/filter](https://docs.tableplus.com/gui-tools/filter), b200:
  - header right-click → **Filter with column**
  - cell right-click → **Quick Filter ▸** (=, <>, IN, NOT IN, …)
  - FK arrow
- [ ] Free tier: max 2 advanced filters — [docs/licensing](https://docs.tableplus.com/utilities/licensing)

### 3.9 Search within results

- [ ] Search and replace within results (b314), distinct from the filter, which runs SQL — #530, #1518, b320

---

## 4. Structure editor (Structure tab, ⌘⌃])

### 4.1 Layout

- [ ] Header: **Name** field (edit it to rename the table), **Primary** token field (add/remove column names to change the PK), **Triggers** button, **Definition** button (CREATE statement; later labelled "**Info**") — #494, #2425 (screenshots), [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table), [docs/constraint](https://docs.tableplus.com/gui-tools/working-with-table/constraint), #3799
- [ ] Bottom bar: Data | Structure, **+ Index**, **+ Column**, and a column-name search field (⌘F) — #494 (screenshot)
- [ ] Two grids: **Columns** (top) and **Indexes** (bottom) — #494
- [ ] Order columns by name or by ordinal position (b514). No drag-to-reorder, because multi-select takes priority — #607
- [ ] Tab moves to the next field (b282). Editing is keyboard-first (no checkboxes) — #1838

### 4.2 Columns grid (editable attributes)

- [ ] PG columns: `#`, **column_name**, **data_type** (dropdown/stepper; includes enums and custom types), **is_nullable** (YES/NO), **check**, **column_default**, **foreign_key**, **comment** — #494 (screenshot), #1963, #3609, #3503
- [ ] MySQL columns: `#`, column_name, **data_type** (full column type, e.g. `bigint(20) unsigned`; unsigned is typed into data_type), is_nullable, column_default, **extra** (e.g. auto_increment, on update CURRENT_TIMESTAMP), foreign_key, comment — #960, #2532, #1838 (screenshots), #812
- [ ] (MY) Per-column charset is viewable but not editable as of 2019. Table **encoding and collation** became editable in b296; MySQL user-level encoding/engine work was also referenced — #1231, #642, b296
- [ ] **column_default** dropdown — #960, #2650 (screenshots), b105:
  - MySQL: EMPTY, NULL, AUTO_INCREMENT, CURRENT_TIMESTAMP, NOW(), **Advanced…**
  - PG: kinds **Sequence / Expression / Constant**
- [ ] New table: one default `id` row. PG uses serial/int with a sequence default; MySQL uses `int` + AUTO_INCREMENT — #2425, #354
- [ ] Add a column with **+ Column**, ⌘I, or double-click a blank row. Delete with the Delete key or right-click Delete. Bulk-edit attributes in the right sidebar (Space) — [docs/column](https://docs.tableplus.com/gui-tools/working-with-table/column)
- [ ] (SL) Alter column via table rebuild (b388); rename column (b282); generated columns (b336)
- [ ] (PG) Show precision/scale and varchar length (b113); include columns in indexes (b546)
- [ ] (MY) Reset auto-increment (b308)

### 4.3 Foreign keys

- [ ] Click the **foreign_key** cell → "Create a foreign key on column" → a popover with **Table**, **Columns** (tokens), **Referenced Table**, **Referenced Columns**, **On Update**, **On Delete**, plus **Delete** and **Ok** buttons — [docs/constraint](https://docs.tableplus.com/gui-tools/working-with-table/constraint), #494, #2532 (screenshots)
- [ ] On Update / On Delete options: **NO ACTION, RESTRICT, CASCADE, SET NULL, SET DEFAULT** — #2532 (screenshot)
- [ ] The FK name is not editable; the default name is used — #540, #417
- [ ] Composite FKs supported later; FKs only after the table exists — #102, #494

### 4.4 Indexes

- [ ] Index grid columns: **index_name**, **index_algorithm** (BTREE, …), **is_unique** (TRUE/FALSE), **column_name** (multi, ordered), **condition** (PG partial index WHERE), **comment** — #494 (screenshot), #157, #1973
- [ ] Add with **+ Index**, ⌘I, or double-click a blank row. Edit inline. Delete with the Delete key — [docs/index](https://docs.tableplus.com/gui-tools/working-with-table/index)
- [ ] MySQL index column length (b228). (PG) INCLUDE columns shown (b546). PG indexes are included in the table creation statement (b546)

### 4.5 Constraints, triggers, other objects

- [ ] NOT NULL via is_nullable, PK via the Primary field, DEFAULT via column_default, FK via the popover — [docs/constraint](https://docs.tableplus.com/gui-tools/working-with-table/constraint)
- [ ] CHECK: a per-column `check` cell exists (PG). Multi-column / table-level CHECK constraints are **not** shown or edited (open FRs), even in the Definition — #1118, #1815, #3759, #3799, #2057
- [ ] UNIQUE constraints appear only as unique indexes — #2723
- [ ] **Triggers** button → trigger grid. Add with **+ Trigger** or double-click a blank row. Edit attributes and the statement inline. Delete with the Delete key. MySQL and PG only (2018) — [docs/trigger](https://docs.tableplus.com/gui-tools/working-with-table/trigger), #330, #364, b104
- [ ] Partitions (PG/MY) are listed as separate tables, so use sidebar group rules. The structure tab of a partitioned parent was empty in 2022 — #3463, #935, #2749, #3167
- [ ] Views — [docs/view](https://docs.tableplus.com/gui-tools/database-objects/view), b296, b348, b204:
  - create: New ▸ View, name it, write the SQL, ⌘S
  - edit: Structure tab shows the SQL definition, editable
  - (PG) pretty definition, change view encoding
- [ ] Materialized views: create (New Materialized View); PG indexes on mat-views not shown — #2815, #3362
- [ ] Functions/procedures — [docs/function](https://docs.tableplus.com/gui-tools/database-objects/function), [docs/procedure](https://docs.tableplus.com/gui-tools/database-objects/procedure), #472, #414:
  - New ▸ Function/Procedure gives a raw SQL editor; the text is sent as-is, without DELIMITER
  - open shows the SQL; edit and ⌘S drops and recreates it
- [ ] Enums (b106). (PG) Sequences open and their creation statement can be copied (b478, b546)
- [ ] Table comment editable in Item Overview (b366) — #2607

### 4.6 Preview / commit

- [ ] Every structure change is staged. Preview with ⌘⇧P, commit with ⌘S, discard with ⌘⇧⌫. ALTERs are grouped into one statement for MY/PG (b388). If an error occurs, "All changes were reverted" — [docs/code-preview](https://docs.tableplus.com/gui-tools/code-review-and-safemode/code-preview), b388, #960, #2945

---

## 5. SQL editor / queries

### 5.1 Editor basics

- [ ] Open with the **SQL** button, ⌘E or ⌘↩ (from a non-SQL tab). It opens with the most recent script. An "SQL Viewer" opens with ⇧⌘O — [docs/query-editor/untitled](https://docs.tableplus.com/query-editor/untitled), [docs/getting-started](https://docs.tableplus.com/getting-started)
- [ ] Bottom bar of the editor — #1205, #3626 (screenshots):
  - ⚙ settings
  - cursor position `line N, column N, location N`
  - **row-limit dropdown** ("No limit" default; e.g. 100/300/500/1000 rows)
  - **Beautify ⌘I** with a dropdown (Uglify ⇧⌘I)
  - **Run Current ⌘↩** with a dropdown (Run All, Save SQL as…)
- [ ] **Run Current** (⌘↩) runs the statement under the cursor or the selection. **Run All** is ⌘⇧↩. The default button (Run Current ↔ Run All) can be switched in the dropdown. The Run Current/Run All shortcuts are remappable (b336) — [docs/query-editor/untitled](https://docs.tableplus.com/query-editor/untitled), [blog: getting started](https://tableplus.com/blog/2018/04/getting-started-with-tableplus.html), #255, #1223, #2030
  - **⚠ conflict**: the docs getting-started page says ⌘↵ = Run All; current docs and the maintainer say ⌘↵ = Run Current
- [ ] Statements are split on `;`; the parser detects transaction/code blocks (b115). The current statement is highlighted (toggle "Highlight current Query")
- [ ] Executed SQL is sent unmodified. There is **no auto-commit wrapping** in the editor, so you type BEGIN/COMMIT yourself — #818, #230
- [ ] **Auto row limit**: appends LIMIT to SELECTs. The footer shows "N rows (auto limited N rows)" — b194, #1205, #2125, #2417
- [ ] Cancel a running query with the **Cancel** button (the Run button turns into Cancel) or ⌘. → opens the process list → **Kill** — [docs/query-editor/untitled](https://docs.tableplus.com/query-editor/untitled), #783, #1752
- [ ] **⚙ settings menu** — #2915, #934, #3626 (screenshots):
  - Font Size ▸
  - Show invisible Characters
  - Wrap lines to Editor Width (b458)
  - Highlight current Query
  - Restore recent Queries; later replaced by **Auto Save SQL Query Tab ▸** with: Do not save the SQL query (always open an empty SQL Query tab) / Only Save the First SQL Query Tab / Save all SQL Query Tabs
  - Key binding ▸ (Run Current/Run All key)
  - Autocomplete options ▸
  - Query params options ▸
  - Result view mode ▸
  - Show result Row Numbers
  - Return on Error
- [ ] Older ⚙ items: show scrollers, "Split results into Tabs (max 100)", "Enable Raise Notice notifications" (PG) — [docs/customize-query-editor](https://docs.tableplus.com/query-editor/customize-query-editor), [docs/split-results-into-tabs](https://docs.tableplus.com/query-editor/split-results-into-tabs), [blog: customize](https://tableplus.com/blog/2018/10/customize-tableplus-experiences-mac.html)
- [ ] **Return on Error**: when on, a multi-statement run stops at the first error; when off it carries on. It is on by default — [docs/customize-query-editor](https://docs.tableplus.com/query-editor/customize-query-editor), #742
- [ ] Editing features:
  - Toggle line comment ⌘/; toggle block comment (b210)
  - Wrap selected text in quotes by typing `'` or `"` (b270)
  - Auto-insert closing braces and quotes (pref)
  - Parentheses highlighting (b330)
  - Code folding (b768)
  - Find (⌘F) and replace in the editor
  - Font size ⌘+ / ⌘−
  - Indent with tabs or spaces, tab width (pref)

  Sources: [docs/shortcut-keys](https://docs.tableplus.com/utilities/shortcut-keys), [blog: build 270](https://tableplus.com/blog/2019/10/whats-new-in-build-270.html), b768, #1509

- [ ] **Multiple carets** — [docs/multiple-carets](https://docs.tableplus.com/query-editor/multiple-carets), b270:
  - ⌘+Click adds a caret
  - ⌃⇧↑ / ⌃⇧↓ add carets above/below
  - ⌘D selects the next instance (works on the word under the caret)
  - ⌘⌃G selects all instances
  - Esc exits
- [ ] **Split pane**: right-click → "Split pane horizontally" or ⌘⇧D. The panes work independently — [docs/split-panes](https://docs.tableplus.com/query-editor/split-panes), #1582
- [ ] Error markers stay until you edit or re-run (b556). Disconnect popup with Reconnect (b568)
- [ ] (PG) RAISE NOTICE shown as notifications (b174)

### 5.2 Autocomplete

- [ ] Suggests databases, tables, keywords, columns and functions (b110). Fuzzy matching. Case-sensitive names are auto-quoted (b101). "add all table fields" helper (b286). Cross-database tables for MySQL (b210). Works with multiple cursors (b282) — [docs/autocomplete](https://docs.tableplus.com/query-editor/autocomplete)
- [ ] **Autocomplete options ▸**: Tables, Keywords, **Disable all suggestions**, **Automatically add a whitespace** (b360), **Automatically prefix schemas** (b240), **Automatically uppercase keywords**, **Automatically insert closing braces and quotes** — #2915 (screenshot)
- [ ] Accept key pref: "**Auto-complete key: Enter or Tab**" (Enter, Tab, or both) — #561, #1745
- [ ] Favorite/Queries **keyword binding**: type the keyword and accept it to insert the saved query (§5.5) — [docs/keyword-binding-favorite](https://docs.tableplus.com/query-editor/keyword-binding-favorite)

### 5.3 Query parameters (variables)

- [ ] Off by default. Turn on with ⚙ → **Query params options ▸ Enable query params**, which also has **Allow searching for variables in Quoted String** and **Change query params Regex** — [docs/query-parameters](https://docs.tableplus.com/query-editor/query-parameters), #934 (screenshot)
- [ ] Regex choices (no custom regex): `:[\w.]+`, `%[\w.]+%`, `\?`, `\$\{[\w.]+\}`, `\$[\w.]+`, `\{\{[\w.]+\}\}` — [blog: call functions](https://tableplus.com/blog/2021/04/tips-how-to-call-function-producedure-using-gui-in-tableplus.html), [blog: build 270](https://tableplus.com/blog/2019/10/whats-new-in-build-270.html)
- [ ] Running a query with variables prompts for their values. ⌘↩ applies them — #1910
- [ ] Comment-driven charts are always on, e.g. `-- TablePlus BarChart x: a, y: b` above a SELECT — [docs/query-parameters](https://docs.tableplus.com/query-editor/query-parameters), b512

### 5.4 Results

- [ ] **Streaming results and async loading**: rows appear while the query is still running, without blocking the UI. Loading continues in the background (no paging) and can be cancelled — [docs/streaming-results](https://docs.tableplus.com/query-editor/streaming-results-and-async-loading), #547, #245
- [ ] **Split results into tabs**: one tab per result set, up to 100, on by default (b100). **Result view mode ▸** also offers "**Append all results in one tab**" — [docs/split-results-into-tabs](https://docs.tableplus.com/query-editor/split-results-into-tabs), #2228, [blog: multiple results](https://tableplus.com/blog/2018/08/show-multiple-results-separately.html), #2465
  - Very large result counts fall back to plain text
- [ ] **Pin results** (b336). Hide the query result with a shortcut (b472). Row numbers in results (b432)
- [ ] Results footer: execution time (e.g. "6 ms"), row count, **Show message** (per-statement messages such as "Query 1 OK: … rows affected"), **Export…** — #1205 (screenshot), #1040
- [ ] **Editable results**: double-click or use the right sidebar. Rows can be added, deleted and duplicated in results (b210). Results from JOINs and aliases are editable when the source table and its PK can be detected (b600); otherwise you get "Unable to detect table". Changes are committed with ⌘S like the grid — [docs/working-with-query-results](https://docs.tableplus.com/query-editor/working-with-query-results), b210, b600, #2641
- [ ] Export results via the **Export…** button or right-click **Export result…**: CSV, JSON, SQL (SQL uses `untitled` when the table is unknown); Excel since b718. Multiple results can be exported in tab mode (b700) — [docs/working-with-query-results](https://docs.tableplus.com/query-editor/working-with-query-results), #894, b718, b700
  - Only the fetched/limited rows are exported — #2916
- [ ] EXPLAIN: no dedicated Explain button or visual plan. The user types EXPLAIN (FRs for a hotkey and a GUI are open). Safe mode Alert/Safe 2 exempt EXPLAIN — #3377, #3409, #2627
- [ ] The console log records executed queries (§11.3)

### 5.5 Queries (favorites), history, files

- [ ] **Queries/Favorites** pane — [docs/keyword-binding-favorite](https://docs.tableplus.com/query-editor/keyword-binding-favorite), b200, b104:
  - **New ▸ File…** or right-click → **Add to favorite** (from the editor or History) opens a popup editor with **name + binding keyword**; ⌘S saves
  - item menu: **Run** (in a new tab), **Copy** (⌘C), **Import…** (via the SQL import wizard), **Open in a new tab**, **Insert to SQL Editor**, **Edit/Rename**, **Delete**, **Show in Finder**
  - folders: **New ▸ Folder…**, **Add Folder to Favorites…** (import a disk folder); drag queries into folders
- [ ] Favorites settings (sidebar dropdown and prefs): double-click runs by default; "**Double click to Insert Favorite**" switches it to insert; "**Auto save Favorite**" — [docs/keyword-binding-favorite](https://docs.tableplus.com/query-editor/keyword-binding-favorite), #1115
- [ ] Queries are stored as `.sql` files in a configurable location (Preferences → Locations) — #1745
- [ ] **History** pane logs every query run from the editor — [docs/query-history](https://docs.tableplus.com/query-editor/query-history), b436, b200, b818, #1712:
  - double-click inserts it (per docs) or, since b436, opens a new SQL tab
  - menu: Copy, Run, Open in a new tab, Insert to SQL Editor, Add to favorite, Delete, **Clear all history**, Show in Finder
  - copy multiple entries (b200)
  - search (b308), much faster with **Match Case / Whole Word / Regex** (b818)
  - keep history for 1, 2 or 3 years, or clear history older than a date (b818)
  - auto-limited queries are recorded as typed
- [ ] Files — [docs/shortcut-keys](https://docs.tableplus.com/utilities/shortcut-keys), [docs/query-editor/untitled](https://docs.tableplus.com/query-editor/untitled), b204, b436, b182, b524, b558:
  - **Open** ⌘O; `.sql` files always open in a new tab; any file can be opened (b436); Gzip files open (b182)
  - **Save SQL as…** in the Run dropdown; **Save selected SQL statements as** ⇧⌘S
  - ⌘S in SQL Query triggers save when auto-save is off (b121, #1438)
  - improved create/save UX (b524); large-file freeze fixed (b558)
- [ ] Pref "**Automatically save queries while editing**" (on) — [docs/sql-editor](https://docs.tableplus.com/preferences/general/sql-editor), #1745

### 5.6 Formatting

- [ ] **Beautify** (⌘I) / **Uglify** (⇧⌘I) work on the selection or the current statement (b276). They respect the indent setting and uppercase keywords. Since b578 an error is shown instead of nothing happening — [docs/query-reformat](https://docs.tableplus.com/query-editor/query-reformat), b276, b578, #1392
- [ ] Optional **SQL Formatter plugin**: Plugin → SQL Format, ⌃I — [docs/plugin](https://docs.tableplus.com/utilities/plugin)

---

## 6. Import / export (TablePlus-native, not binaries)

### 6.1 Export

- [ ] Entry points — [docs/import-and-export](https://docs.tableplus.com/gui-tools/import-and-export), [blog: import-export](https://tableplus.com/blog/2018/10/import-export-database.html), #797, #1633, b360:
  - **File → Export…** (⌘⇧E)
  - sidebar right-click **Export…** (all selected tables; ⌘A then Export)
  - grid right-click **Export current page…** (current page only)
  - results **Export…** / **Export result…**
  - filter bar **Export** (filtered result)
- [ ] Formats tabbed **CSV | JSON | SQL** — #1475, #2723 (screenshots). Excel added for results, tables, multiple tables and multiple results (b718). HTML export listed in b100. The getting-started blog lists CSV, JSON, SQL or Excel
- [ ] **CSV options**, with defaults from the 2019 screenshot — #1475:
  - **Convert NULL to EMPTY** ☑
  - **Convert line break to space** ☐
  - **Put field names in the first row** ☑
  - **Delimiter** (`,` / Tab / `;`)
  - **Swap**/quote: `"`, `'`, Space, **Quote if needed** (default since b222)
  - **Line break** (`\n`, `\r\n`, `\r`)
  - **Decimal** (`.` / `,`)
  - plus custom decimal delimiter (b114) and respect for column order (b233, b320)
- [ ] **SQL options** — #2723 (screenshot), b296, b308, b388:
  - **Drop table if exists**
  - **Include table structure**
  - **Include table content** ☑
  - **Compress the file using Gzip**
  - footnote: not all DB features are supported, use File → Backup… for full compatibility
  - INSERTs are chunked at 250 KB (b296); MySQL virtual/generated columns are skipped (b308, b388)
  - PG FKs are exported (b182); the dump sets the session time zone (#1336)
- [ ] **JSON**: pretty-printed array of objects (no JSONL) — #2596, #1959
- [ ] Multi-table / "advanced export" (b608): the dialog lists **all databases** on the left with their tables. Users with huge DB counts complained, and an "**Export Single Table**" option remains — #3627, #3755
  - older modal fields: "Select field to export", "Export where" — #158
- [ ] Remembers favourite export options (b182). Notification and "show containing folder" popup after export (pref "Show open containing folder popup after exporting data") — b182, #3583 (screenshot)
- [ ] File encoding follows View Using Encoding — #1219
- [ ] Not exported: views' bodies, triggers, events, PG indexes (FRs). Use Backup instead — #1245, #2439, #2723

### 6.2 Import

- [ ] Entry points: **File → Import ▸ From CSV…**, **From SQL Dump…** (b142); JSON import (b718); sidebar or grid right-click **Import ▸** — [docs/import-and-export](https://docs.tableplus.com/gui-tools/import-and-export), [docs/getting-started](https://docs.tableplus.com/getting-started)
- [ ] **Import CSV Wizard** fields — #420 (screenshots, 2021–2024), #2208, b222, b119, b402:
  - **Table** dropdown and a **Create new table** checkbox (create a table from the CSV, b119)
  - file name and size; preview of the **first 100 rows**
  - **First line is header** ☑
  - **matching strategy** dropdown: Match Columns by Name and Order (default) / … – Case insensitive / Match Columns by Name / … – Case insensitive / Match Columns by Order
  - a per-column header dropdown mapping each CSV column to a table column or **Do not import**
  - status line "CSV n columns, table m columns, matched k columns"
  - **Delimiter**, **Quote**, **Encoding** (utf8 …)
  - **Insert option**: **STOP ON ERROR** / **REPLACE** (MY) / **UPDATE ON DUPLICATE** — "update rows from CSV" (b402)
  - **Cancel**, **Import**
- [ ] CSV semantics — #1523, #3691, #3200, #1780, #1264, b164:
  - quoted `""` = EMPTY; unquoted empty → EMPTY/NULL (inconsistent across builds)
  - "Autodetect column datatype" when creating a table
  - rows are imported in chunks (no ignore-and-continue)
  - the console is suppressed during import
  - error details are shown
- [ ] **Import SQL dump**: the app parses and sends statements (with a preview → Continue step). It falls back to ASCII / Windows-1252 if the file isn't UTF-8 (b154). Opens .gz files (b182). Not the native binaries; for big or complex dumps use Restore — [blog: import-export](https://tableplus.com/blog/2018/10/import-export-database.html), #1551, #3446
- [ ] Press ⌘R after an import to reload — [docs/getting-started](https://docs.tableplus.com/getting-started)
- [ ] Paste CSV/TSV straight into the grid (⌘V) as a lightweight import — #224, #1541

---

## 7. Backup / restore (native dump binaries)

- [ ] Opened from the welcome screen **Backup**/**Restore** buttons or **File → Backup… / Restore…** — [docs/backup-and-restore](https://docs.tableplus.com/gui-tools/backup-and-restore)
- [ ] **Backup database** window — #884 (screenshot, 2019), #945 (screenshot), b436:
  - column 1: connection list (grouped)
  - column 2: database list with search (b436)
  - column 3: **driver/binary version** dropdown ("Choose version", e.g. "MySQL 5.7")
  - **Add option…** dropdown that adds flag chips, e.g. `--compress`, `--routines`, `--lock-tables=false`; free-typed flags are allowed (e.g. `--skip-definer`, `--no-privileges`)
  - **Compress file using Gzip**
  - **File name** pattern ("Click to change")
  - **Start backup…** → pick a folder
- [ ] File-name pattern tokens: connection name, database name, table name, tag, year, month, month pretty, day, hour, minute, second, `-`, `_`, `/`, space. Date and time values are zero-padded (b246) — #945 (screenshot), b233
- [ ] **Restore**: choose connection, database and options, then the file → **Start Restore** — [docs/backup-and-restore](https://docs.tableplus.com/gui-tools/backup-and-restore), [blog: import-export](https://tableplus.com/blog/2018/10/import-export-database.html)
- [ ] Uses **bundled official binaries** (pg_dump/pg_restore, mysqldump/mysql). PostgreSQL 11 through 18 and MySQL 5/8 (incl. ARM) binaries were added over time. The user picks the version to match the server — #203, #2994, #884, #400, b142, b174, b233, b286, b348, b396, b406, b472, b574, b640
- [ ] **MySQL and PostgreSQL only**. SQLite has no backup ("just copy the file"). No per-table selection — #400, #1244, #1553, #2406
- [ ] Options are remembered: flags, gzip and driver version (b233). It works over SSH, socket and SSL (b228, b370). MySQL `--all-databases` (b246). Respects safe mode (b330). Notification when done (b194)
- [ ] No progress bar, because the native binaries don't report progress — #1926, #2121

---

## 8. Database and server management

### 8.1 Databases and schemas

- [ ] **⌘K** opens the database list (search, auto-select first). Double-click opens it in a new workspace; right-click → Open in current workspace / Copy Name (b600) / Drop… — [docs/database](https://docs.tableplus.com/gui-tools/untitled), [docs/multi-tabs](https://docs.tableplus.com/gui-tools/the-interface/multi-tabs-workspaces-windows), #1675, b600
- [ ] **New database**: the **+** button → **name, encoding, collation** → **Create** — [docs/database](https://docs.tableplus.com/gui-tools/untitled), #861, b210
- [ ] **Rename database**: right-click → new name → Rename, only where the server supports it. **MySQL is not supported** — [docs/database](https://docs.tableplus.com/gui-tools/untitled), #1028, #2333
- [ ] **Drop database**: right-click → **Drop…**, or select it and press **−** → **Drop**. One at a time (multi-drop is an open FR) — [docs/database](https://docs.tableplus.com/gui-tools/untitled), #2651
- [ ] (PG) **New Schema…** from the sidebar + menu — #2815
- [ ] Opening a DB via ⌘P / ⌘K runs `USE db` on MySQL — #1832
- [ ] SQLite: create a new DB file by choosing a folder in the connection form — #947

### 8.2 Users and privileges

- [ ] Opened from **Connection → User Management…** or the toolbar button — [docs/user-management](https://docs.tableplus.com/gui-tools/user-management), [blog: manage users](https://tableplus.com/blog/2019/08/how-to-manage-user-mysql-tableplus-gui.html), b170
- [ ] The docs say MySQL only; PostgreSQL user management arrived in b348 and PG table privileges in b718
- [ ] Left: list of users with **+** / **−** buttons (delete is also on right-click) — [docs/user-management](https://docs.tableplus.com/gui-tools/user-management), [blog: build 270](https://tableplus.com/blog/2019/10/whats-new-in-build-270.html), b194, #3859
- [ ] Right side:
  - username and password
  - **Global Privileges** tab
  - **Database Privileges** tab (pick a DB, then grant; "Granted Privileges" list)
  - **resource limits**: Max Updates, Max Connections, Max Questions (per hour)
  - **Apply**
  - admin privileges include CREATE TEMPORARY TABLES (b270); per-database user management (b194)
- [ ] There is no "clone user / add host" helper — #2045
- [ ] Respects safe mode — b194

### 8.3 Process list / running queries

- [ ] **Connection → Process List…** (⌘.) shows the "Running queries" window: select a query → **Kill**. It uses a separate connection — [docs/query-editor/untitled](https://docs.tableplus.com/query-editor/untitled), #343, #1875, b600, #3768
  - Older builds showed only TablePlus's own queries. MySQL needs `information_schema.processlist` access — #1332
  - Database filter (b600); column sort (2025)
- [ ] Killing unresponsive queries works for PG and MySQL (b233 and later) — #619, #783

### 8.4 Metrics board (dashboards)

- [ ] Open it with the toolbar **Metrics Board** button. Boards are listed in a sidebar (New… board) — [docs/metrics-board](https://docs.tableplus.com/gui-tools/metrics-board), [blog: mini tools](https://tableplus.com/blog/2023/12/how-to-design-mini-tools-using-tableplus.html), b454, b512, b520, b546, b730
- [ ] Add objects with **+**: **Bar chart, Line chart, Pie chart, Scoreboard, Data table, Input field, Dropdown, Button**
- [ ] Each object has an SQL query and a refresh rate: After xx seconds / **Refresh on event** / Do not refresh
- [ ] Inputs set a variable and are wired to objects by dragging the green dot. The target must use Refresh on Event
- [ ] Board import/export, with an optional password (b570, b730) — b570, #3373

---

## 9. Code review, commit flow, safe mode in practice

- [ ] Every GUI change is pending until committed: data edits, row insert/delete, structure, truncate/drop, and new view/function. SQL-editor statements bypass this — [docs/code-review-and-safemode](https://docs.tableplus.com/gui-tools/code-review-and-safemode), [docs/commit-changes](https://docs.tableplus.com/gui-tools/code-review-and-safemode/commit-changes)
- [ ] **Preview** (⌘⇧P, eye icon) shows the generated SQL for all pending changes — [docs/code-preview](https://docs.tableplus.com/gui-tools/code-review-and-safemode/code-preview)
- [ ] **Commit** (⌘S, the commit icon) sends the statements wrapped in **BEGIN … COMMIT**, then refreshes the open tables. It does not reload every table (b256) — #230, #818, b256
- [ ] Pending changes across **multiple tabs** of the same workspace are committed together in one transaction — #259, #1600
- [ ] On error the changes are reverted, with the message "All changes were reverted" — #22, #960
- [ ] **Discard** (⌘⇧⌫, ⓧ icon) drops all pending GUI changes — [docs/discard-changes](https://docs.tableplus.com/gui-tools/code-review-and-safemode/discard-changes)
- [ ] Change markers: **red** soft-deleted rows, **green** new rows, **tangerine** modified cells. The colours are customizable — [blog: customize-colors](https://tableplus.com/blog/2019/10/customize-colors.html), [docs/fonts-and-colors](https://docs.tableplus.com/preferences/fonts-and-colors)
- [ ] Commit selection behaviour: a commit deselects the row being edited — #2979
- [ ] Safe-mode prompts on commit and query run per §1.12 (the alert dialog; password or Touch ID dialog). It was redesigned in b110 — b110

---

## 10. Preferences and keyboard shortcuts

### 10.1 Preferences window

Tabs (2023+): **General, Locations, Fonts & Colors, Security, Keymap** — #3583, #3596 (screenshots). Older: General, Fonts & Colors, Crash & Security, Keymap — #1570. Values below come from 2020–2023 screenshots; ☑ means on in the screenshot, which is usually the default.

**General → Application**

- [ ] Use loading wallpaper ☑ — [docs/application](https://docs.tableplus.com/preferences/general/application)
- [ ] Receive Beta Updates ☐ — [docs/application](https://docs.tableplus.com/preferences/general/application)
- [ ] Turn off auto update and update notification ☐ — [docs/application](https://docs.tableplus.com/preferences/general/application)
- [ ] Reopen closed workspaces at startup (☑ default since b556) — [docs/application](https://docs.tableplus.com/preferences/general/application), b556

**General → Table Data**

- [ ] Auto hide table scrollers — [docs/table-data](https://docs.tableplus.com/preferences/general/table-data), #3583
- [ ] Alternating row background colors ☑ — [docs/table-data](https://docs.tableplus.com/preferences/general/table-data)
- [ ] Estimate count when table has more than **500000** rows ☑ — [docs/table-data](https://docs.tableplus.com/preferences/general/table-data), #1745
- [ ] Allows expansion Tool Tips ☐ — #1745
- [ ] Show open containing folder popup after exporting data ☑ — #3583
- [ ] Older items: Disable scroll elastically, Auto expand text column, line height / font size — [blog: customize](https://tableplus.com/blog/2018/10/customize-tableplus-experiences-mac.html)

**General → SQL Editor**

- [ ] Automatically save queries while editing ☑ — [docs/sql-editor](https://docs.tableplus.com/preferences/general/sql-editor)
- [ ] Automatically UPPERCASE keywords ☑ — [docs/sql-editor](https://docs.tableplus.com/preferences/general/sql-editor)
- [ ] Automatically insert closing braces and quotes ☑ — #561
- [ ] **Auto-complete key**: Enter or Tab (or just one) — #561
- [ ] **Indent with**: Spaces / Tabs, width "4 spaces" — #1475, #1745

**General → CSV default options**

These apply to copy, export and import — [docs/csv-file](https://docs.tableplus.com/preferences/general/csv-file), #1745, #1570:

- [ ] Delimiter: Tab / `,` / `;` (Tab in the screenshots)
- [ ] Swap: `"` / `'` / Space / Quote if needed
- [ ] Line break: `\n` / `\r\n` / `\r`
- [ ] Decimal: `.` / `,`

**General → Others / Connections**

- [ ] Query time out **300 s** — [docs/preferences/connections](https://docs.tableplus.com/preferences/general/connections), #3583
- [ ] Keep connection alive ☑ (ping every 30 s) — [docs/preferences/connections](https://docs.tableplus.com/preferences/general/connections)
- [ ] Report crashes ☑; older wording "Disable the Crash Report" — [docs/crash-and-security](https://docs.tableplus.com/preferences/crash-and-security)

**General → other sections**

- [ ] Data Decoder: Automatically convert PostGIS geometry to WKT ☑ — #1676, #561
- [ ] New tab: "**Command + T should open**": SQL Query Editor tab / empty tab — #3583, #3778
- [ ] Editing (older "Behaviours" tab): Auto convert empty Numeric, UUID, JSON or Array to NULL ☑ — #1475
- [ ] Editing (older "Behaviours" tab): Connection startup: Show the Welcome Screen / auto-open a connection, plus "Prompt the input form" — #1475
- [ ] Not in a screenshot: lazy-load default, history retention (b818), LLM plugin toggle — [docs/llm-plugin](https://docs.tableplus.com/llm-plugin), b818

**Locations**

- [ ] Data locations for connections and queries (Dropbox/iCloud-capable) — b296, #193, #1745

**Fonts & Colors**

- [ ] Theme: **Light / Dark / Auto** (follows the system) — [docs/fonts-and-colors](https://docs.tableplus.com/preferences/fonts-and-colors), b136, b494, b802
  - custom themes import/export (b494); four built-in themes plus custom themes (b802)
- [ ] SQL Editor font and size, line height (b348). Syntax colours for: Comment, Number, Single quote string, Double quote string, Backtick quote string, Bracket quote string, Reserved keyword, Current query, Selected background, Error highlight, Others; invisible-character colour (b360) — [docs/fonts-and-colors](https://docs.tableplus.com/preferences/fonts-and-colors), [blog: customize-colors](https://tableplus.com/blog/2019/10/customize-colors.html)
- [ ] Data Table font, size and **padding** (b222). Colours for Soft deleted row, Modified value, New row; date text colour (b446). Left sidebar font (b432). All macOS fonts selectable (b222) — [docs/fonts-and-colors](https://docs.tableplus.com/preferences/fonts-and-colors)

**Security** (formerly Crash & Security) — #2669, #3596 (screenshots), b406, b396:

- [ ] **Unlock Safe Mode with Touch ID**
- [ ] **Default Safe Mode for new connection** (default Silent Mode)
- [ ] **Passcode**: Enable Passcode / Change Passcode…, Unlock Passcode with Touch ID, **Auto lock if away for** (e.g. 1 / 5 minutes) — the master passcode is asked at startup and after being away
- [ ] **AI Assistant**: Enable Assistant Tab on the Right Sidebar ☑ (in 2025; the ChatGPT view was off by default from b562)

**Keymap**

- [ ] Click a shortcut and type to remap; clear to reset — [docs/keymap](https://docs.tableplus.com/preferences/keymap), b210, b336
  - Navigate: Select Next Pane, Select Previous Pane, Select Next Tab, Select Next Workspace, Select Previous Workspace
  - SQL Editor: Toggle Line Comment, Toggle Block Comment, Increase Font Size, Decrease Font Size
  - Run Current / Run All (also via ⚙ Key binding)
- [ ] Not remappable: ⌘R reload, ⌘0 toggle list — #3802

### 10.2 Keyboard shortcuts — full table

Primary sources: [docs/shortcut-keys](https://docs.tableplus.com/utilities/shortcut-keys) and [blog: shortcut-keys](https://tableplus.com/blog/2018/01/shortcut-keys.html). Rows marked with † come from changelog or maintainer comments.

| Shortcut     | Action                                                                                                                     | Scope / source                                                                        |
| ------------ | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| ⌘N           | New window / new workspace / open a new connection                                                                         | Global                                                                                |
| ⌘,           | Preferences                                                                                                                | Global                                                                                |
| ⌘Q           | Quit                                                                                                                       | Global                                                                                |
| ⇧⌘O          | New SQL Viewer                                                                                                             | Global                                                                                |
| ⌘W           | Close tab/window                                                                                                           | Workspace                                                                             |
| ⌘S           | Commit changes (in SQL tab: save file when auto-save is off)                                                               | Workspace; #1438                                                                      |
| ⇧⌘P          | Preview pending SQL                                                                                                        | Workspace                                                                             |
| ⇧⌘⌫          | Discard pending changes                                                                                                    | Workspace                                                                             |
| ⌘T           | New tab (SQL Query by default)                                                                                             | Workspace; #3778                                                                      |
| ⌘L           | Plugin manager                                                                                                             | Workspace                                                                             |
| ⇧⌘C          | Toggle console log (docs) **⚠** also "Copy cell value" (b446); blog lists ⌃⌘C for console                                  | Workspace                                                                             |
| ⌘.           | Cancel / Process List (blog, #343); docs label: "Run custom script"                                                        | Workspace                                                                             |
| ⌘R           | Reload workspace (whole workspace, incl. enums since 2024)                                                                 | Workspace; #2462, #2630                                                               |
| ⌥⌘R †        | Reload current tab / Refresh (grid menu)                                                                                   | b222, #2459                                                                           |
| ⇧⌘R †        | Force reconnect                                                                                                            | b136, #2756                                                                           |
| ⌘[ / ⌘]      | Previous / next tab                                                                                                        | Workspace                                                                             |
| ⇧⌘[ / ⇧⌘]    | Previous / next pane (blog: ⌥⌘[ / ⌥⌘])                                                                                     | Workspace                                                                             |
| ⇧⌘D          | Split pane horizontally (SQL); split screen right (b600)                                                                   | Workspace                                                                             |
| ⌘1…9         | Go to tab N                                                                                                                | Workspace                                                                             |
| ⌘K           | Switch database                                                                                                            | Workspace                                                                             |
| ⇧⌘K          | Switch connection                                                                                                          | Workspace                                                                             |
| ⌘P           | Open Anything (⌃P / ⌃N move; hold ⌘ on open = new tab)                                                                     | Workspace; b256, #2454                                                                |
| ⌘E           | Open SQL Query editor                                                                                                      | Workspace                                                                             |
| ⌘↩           | Open SQL editor (non-SQL tab) / Run Current (SQL tab) / Quick Look or FK peek (grid cell) / Apply All filters (filter bar) | context-dependent; #1102, #1156                                                       |
| ⇧⌘↩          | Run All (SQL) / Apply All filters with OR (filter bar)                                                                     | #2030, #1577                                                                          |
| ⌘O           | Open SQL file                                                                                                              | SQL tab                                                                               |
| ⌘I           | Beautify (SQL); insert row (grid); insert column/index (structure); insert/duplicate filter (filters)                      | context-dependent                                                                     |
| ⇧⌘I          | Uglify (SQL); remove filter (filters)                                                                                      | context-dependent                                                                     |
| ⌃I           | SQL Formatter plugin / indent                                                                                              | SQL tab                                                                               |
| ⇧⌘S          | Save selected SQL statements as…                                                                                           | SQL tab                                                                               |
| ⌘/           | Toggle line comment                                                                                                        | SQL tab                                                                               |
| ⌘+Click      | Add caret                                                                                                                  | SQL tab                                                                               |
| ⌃⇧↑ / ⌃⇧↓    | Add caret above / below                                                                                                    | SQL tab                                                                               |
| ⌘D           | Select next instance (SQL) / duplicate row (grid)                                                                          | context-dependent                                                                     |
| ⌃⌘G          | Select all instances                                                                                                       | SQL tab                                                                               |
| ⌘+ / ⌘−      | Font size up / down                                                                                                        | #1509                                                                                 |
| ⌘Z / ⇧⌘Z     | Undo / redo                                                                                                                | Workspace                                                                             |
| Middle click | Quick Look                                                                                                                 | Table data                                                                            |
| Space        | Toggle row detail (right sidebar)                                                                                          | Table data                                                                            |
| ⌘C / ⌘V      | Copy rows / paste rows (as new rows)                                                                                       | Table data                                                                            |
| ⌘F           | Row filter (table) / find (SQL editor) / search (welcome)                                                                  | context-dependent; #1760                                                              |
| ⌥⌘F          | Column filter                                                                                                              | Table data                                                                            |
| ⇧⌘F †        | Focus sidebar item search                                                                                                  | b118, #1760                                                                           |
| ⌥+click      | Quick edit menu (Set value: NULL/EMPTY/DEFAULT…)                                                                           | Table data                                                                            |
| ⌥↩ †         | Set Value menu                                                                                                             | b360, #2459                                                                           |
| Tab          | Move to next cell / field while editing                                                                                    | Table data, structure                                                                 |
| ⌃⌘[ / ⌃⌘]    | Data view / Structure view                                                                                                 | [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table)           |
| ⌫            | Delete selected rows / items (staged)                                                                                      | grid, sidebar                                                                         |
| ⌥⌫ †         | Truncate table(s)                                                                                                          | b233                                                                                  |
| ⇧⌫ †         | Delete table CASCADE                                                                                                       | #155, #1459                                                                           |
| ⌘⌫ †         | Delete table(s) ignoring FK checks                                                                                         | b270, #1459                                                                           |
| ⌘↑ / ⌘↓ †    | Move between filters / focus table vs. data                                                                                | #1612, #2974                                                                          |
| ⌘← / ⌘→ †    | Next/previous page (grid); open column/operator popup (filter)                                                             | #2974, #1041                                                                          |
| ⌘B †         | Toggle filter checkbox                                                                                                     | #1612                                                                                 |
| Esc          | Close filters / exit multi-caret                                                                                           | [docs/filter](https://docs.tableplus.com/gui-tools/filter)                            |
| ⌘0 †         | Toggle left sidebar ("toggle list item")                                                                                   | #3337, #3802                                                                          |
| ⇧⌘\\ †       | Show all workspaces                                                                                                        | #150                                                                                  |
| ⇧⌘E †        | Export tables                                                                                                              | [blog: import-export](https://tableplus.com/blog/2018/10/import-export-database.html) |
| ⌘A           | Select all rows / items                                                                                                    | #1633                                                                                 |

---

## 11. Other notable features

### 11.1 Open Anything (⌘P)

- [ ] Spotlight-like fuzzy search across Database, Schema, Table, View, Materialized view, Function, Procedure and **Queries** (saved query files, b388). Since b676 it also searches **app features/commands** — [docs/open-anything](https://docs.tableplus.com/gui-tools/open-anything), b388, b676, b115, b132, #3309
- [ ] Opening a schema switches to it. The opened table is highlighted in the sidebar
- [ ] Navigate with ↑/↓ or ⌃P/⌃N. Return or double-click opens; hold ⌘ to open in a new tab — b256
- [ ] With lazy-load on, only loaded schemas are searched — #3309, #3580

### 11.2 Item overview (table info / stats)

- [ ] Grid of all items in the schema. Columns (MySQL, 2021): **name, schema, kind, charset, collation, engine, estimated_row, total_size, data_size, index_size, comment** — #1695 (screenshot), #219, b366, b120, b256
  - PG adds owner — #219, #1937
  - **comment is editable**; double-click opens the item
- [ ] Opened from the "Tables" header click or right-click → Item overview / Overview items. Sorting is kept on refresh (b656) — [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table), b656
- [ ] View table info in the new update (2023) — #3159

### 11.3 Console log

- [ ] Toggle with ⇧⌘C or the toolbar. It records executed queries, with a filter for **Meta queries only / Data queries only / All queries** and a clear (trash) button. It has syntax highlighting (b320), and its show/hide state is remembered (b228) — [docs/console-log](https://docs.tableplus.com/gui-tools/the-interface/console-log), b125

### 11.4 Plugins (⌘L, JavaScript)

- [ ] **Plugin → Manage Plugins…** (⌘L) — [docs/plugin](https://docs.tableplus.com/utilities/plugin), #44, #636, #2012, b330, org repos
  - Official plugins:
    - **SQL Formatter** (Plugin → SQL Format, ⌃I)
    - **Dump Table** (Copy Script As: CREATION / DROP / TRUNCATE / Laravel Migration)
    - **Open URL**
    - **Diagram** (generate an ER diagram from the sidebar; export PNG/SVG)
    - dummies data, copy-as-update-statement, uuid-generator
  - A JS API exposes the context: selected rows, clicked column, execute, alert, and so on — #159, #1845
  - Third-party plugins are not officially supported

### 11.5 ER diagram

- [ ] There is no built-in ERD before 2026; the plugin draws one. b718 added a "**native diagram**" — #636, #2137, b718

### 11.6 AI / LLM, MCP

- [ ] LLM chat on the right sidebar (b624). Only the table/view **structure** is sent to the LLM, never row data. Providers: OpenAI, Anthropic, GitHub Copilot, DeepSeek, local LLMs, Codex CLI. Custom agents (b742) — [docs/llm-plugin](https://docs.tableplus.com/llm-plugin), b624, b718, b742
- [ ] **TablePlus MCP server** (b718); one-click install into Claude Code / Codex / Cursor / VS Code (b818). Safe-mode exemptions were initially ignored for MCP queries — #3902, #3909

### 11.7 Misc

- [ ] **View Using Encoding…** (Connection menu) re-decodes table data with a chosen encoding — [docs/table](https://docs.tableplus.com/gui-tools/working-with-table/table), [docs/troubleshooting](https://docs.tableplus.com/utilities/troubleshooting)
- [ ] Notification Center (b818). Push notifications for long exports/imports/backups (b182, b194)
- [ ] Deep-link opening of rows (b570); CLI `open -a TablePlus` (§1.10)
- [ ] DBngin companion app for local DB servers — [docs/dbngin](https://docs.tableplus.com/utilities/dbngin)
- [ ] License: perpetual, with 1 year of updates; free tier limits as in §0 — [docs/licensing](https://docs.tableplus.com/utilities/licensing)
- [ ] Crash reports go to AppCenter with 90-day retention, and can be opted out — [docs/master](https://docs.tableplus.com/)

---

## 12. Gaps and unknowns (not verifiable from public sources)

- Exact PostgreSQL SSL-mode list and the contents of the PG "Other options" dropdown: no screenshot found. The MySQL lists are confirmed.
- Exact SQLite filter operator labels; presumably the same as MySQL.
- Clone-table dialog fields (structure only vs data) and truncate-dialog option labels. "Cascade" and "disable foreign key check" are mentioned in issues, but the exact UI was not seen.
- Current (2026) label set of Copy Rows As; the "Excel" export options; the "advanced export" dialog layout (b608).
- Key bindings for back/forward navigation (b586) and "hide query result" (b472).
- Result-limit dropdown values other than "No limit", 100, 300 and 500.
