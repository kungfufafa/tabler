# TablePlus export fixtures

`.tableplusconnection` files for the TablePlus import (`src-tauri/src/tableplus/`). Every value
in them is made up.

They were written on Windows by TablePlus's own code: a PowerShell script loaded
`C:\Program Files\TablePlus\TablePlus.exe` (TablePlus for Windows, October 2026 build) and called
the same steps its `ConnectionService.EncryptAndWrite` takes on export, `SimpleJSON.JSONEncoder.Encode`
on the connection list, then UTF-8, then `RNCryptor.Encryptor.Encrypt(bytes, password)`. The
connection objects carry the keys `Connection.ExportingDictionary` writes.

| File                                                   | Password      |
| ------------------------------------------------------ | ------------- |
| `windows-export.tableplusconnection`                   | `tabler-test` |
| `windows-export-empty-password.tableplusconnection`    | (empty)       |

Both hold the same tree:

- group `Clients` (with a group image), holding
  - `Orders (prod)`: PostgreSQL over SSH, database and SSH passwords, `tLSMode` 2, startup SQL
  - group `Reports`, holding `Sample file`: SQLite
- `Shop`: MySQL at the top level
