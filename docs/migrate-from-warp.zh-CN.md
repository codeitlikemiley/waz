# Migrate settings to Waz
[English](./migrate-from-warp.md) · [日本语](./migrate-from-warp.ja.md)
This article is for those who want to change the **setting class configuration** (custom shortcut keys, themes, workflow, MCP configuration, etc.) from the historical installationUsers brought to Waz.
There are two possible "sources", **the security levels of the two are different**, this article is divided into two sections. If there are both sides,**Please migrate to OpenWarp first, and then consider migrating to Warp**.
1. **OpenWarp** – the former name of Waz himself.2. **Upstream [Warp](https://github.com/warpdotdev/warp)** - the project forked by Waz.
This article intentionally does not cover command history, SQLite databases, Drive objects, or any credentials. Either of theseBind to the local machine (Keychain / DPAPI / libsecret), or the schema is strongly coupled with the other party, and cross overNot safe.
---

## Disk layout overview
Waz (and OpenWarp / upstream Warp) divides disk status into **three categories of directories**:
- **config** —— `settings.toml`、`keybindings.yaml`
- **data** —— `themes/`、`workflows/`、`launch_configurations/`、`tab_configs/`
- **home dotfile** —— `.mcp.json`、`skills/`

The three types of directories on macOS all converge to the same home dotfile directory (`~/.warp/`, `~/.openwarp/`Or `~/.waz/`); on Linux, it is divided into **three different locations** according to the XDG specification, and on WindowsEquivalent layout points for the `directories` crate. The following migration script will put each file by platform intoThe right target.
### Waz target path
| Categories | macOS | Linux | Windows ||---|---|---|---|
| config | `~/.waz/` | `${XDG_CONFIG_HOME:-~/.config}/waz/` | `%LOCALAPPDATA%\waz\Waz\config\` |
| data | `~/.waz/` | `${XDG_DATA_HOME:-~/.local/share}/waz/` | `%APPDATA%\waz\Waz\data\` |
| home dotfile | `~/.waz/` | `~/.waz/` | `%USERPROFILE%\.waz\` |

### OpenWarp source path
| Categories | macOS | Linux | Windows ||---|---|---|---|
| config | `~/.openwarp/` | `${XDG_CONFIG_HOME:-~/.config}/openwarp/` | `%LOCALAPPDATA%\openwarp\OpenWarp\config\` |
| data | `~/.openwarp/` | `${XDG_DATA_HOME:-~/.local/share}/openwarp/` | `%APPDATA%\openwarp\OpenWarp\data\` |
| home dotfile | `~/.openwarp/` | `~/.openwarp/` | `%USERPROFILE%\.openwarp\` |

### Upstream Warp source path
| Categories | macOS | Linux | Windows ||---|---|---|---|
| config | `~/.warp/` | `${XDG_CONFIG_HOME:-~/.config}/warp-terminal/` | `%LOCALAPPDATA%\warp\Warp-Terminal\config\` |
| data | `~/.warp/` | `${XDG_DATA_HOME:-~/.local/share}/warp-terminal/` | `%APPDATA%\warp\Warp-Terminal\data\` |
| home dotfile | `~/.warp/` | `~/.warp/` | `%USERPROFILE%\.warp\` |

> The directory name `warp-terminal` under Linux is consistent with the Linux software package name (for example, under Debian/Ubuntu> `/opt/warpdotdev/warp-terminal/`). The organization folder name on Windows may vary depending on> The packaging method varies; if you are in `%APPDATA%\warp\Warp-Terminal` (or `%LOCALAPPDATA%\warp\Warp-Terminal`)> Not found, please check the `%APPDATA%` / `%LOCALAPPDATA%` path actually used by Warp.
---

## 1. Migrated from OpenWarp (recommended path for old users)
OpenWarp was Waz before it was renamed. Rename submission (`feat: rename project Warp/OpenWarp → Waz`)Only the identifier and disk path name have been changed. **The format and schema of the configuration file have not changed at all**. The following filesYou can copy it directly.
### What can be copied
| File/Directory | Category | What to Control ||---|---|---|
| `settings.toml` | config | Public settings (TOML settings file). || `keybindings.yaml` | config | Custom shortcut keys. || `themes/` | data | Custom theme. || `workflows/` | data | Custom workflow. || `launch_configurations/` | data | Launch configurations. || `tab_configs/` | data | Tab configuration. || `.mcp.json` | home dotfile | MCP server configuration. || `skills/` | home dotfile | Agent skills。 |

### Operation steps
> **Turn off Waz** before copying to prevent any process from holding these files.
**macOS**

```sh
mkdir -p "$HOME/.waz"
for f in settings.toml keybindings.yaml themes workflows launch_configurations tab_configs skills .mcp.json; do
  if [ -e "$HOME/.openwarp/$f" ] && [ ! -e "$HOME/.waz/$f" ]; then
    cp -R "$HOME/.openwarp/$f" "$HOME/.waz/$f"
  fi
done
```

**Linux**

```sh
src_config="${XDG_CONFIG_HOME:-$HOME/.config}/openwarp"
src_data="${XDG_DATA_HOME:-$HOME/.local/share}/openwarp"
src_home="$HOME/.openwarp"

dst_config="${XDG_CONFIG_HOME:-$HOME/.config}/waz"
dst_data="${XDG_DATA_HOME:-$HOME/.local/share}/waz"
dst_home="$HOME/.waz"
mkdir -p "$dst_config" "$dst_data" "$dst_home"

copy() {
  if [ -e "$1/$3" ] && [ ! -e "$2/$3" ]; then
    cp -R "$1/$3" "$2/$3"
  fi
}

copy "$src_config" "$dst_config" settings.toml
copy "$src_config" "$dst_config" keybindings.yaml
copy "$src_data"   "$dst_data"   themes
copy "$src_data"   "$dst_data"   workflows
copy "$src_data"   "$dst_data"   launch_configurations
copy "$src_data"   "$dst_data"   tab_configs
copy "$src_home"   "$dst_home"   .mcp.json
copy "$src_home"   "$dst_home"   skills
```

**Windows(PowerShell)**

```powershell
$src_config = "$env:LOCALAPPDATA\openwarp\OpenWarp\config"
$src_data   = "$env:APPDATA\openwarp\OpenWarp\data"
$src_home   = "$env:USERPROFILE\.openwarp"

$dst_config = "$env:LOCALAPPDATA\waz\Waz\config"
$dst_data   = "$env:APPDATA\waz\Waz\data"
$dst_home   = "$env:USERPROFILE\.waz"
New-Item -ItemType Directory -Force -Path $dst_config, $dst_data, $dst_home | Out-Null

function Copy-IfMissing($srcDir, $dstDir, $name) {
  $from = Join-Path $srcDir $name
  $to   = Join-Path $dstDir $name
  if ((Test-Path $from) -and -not (Test-Path $to)) {
    Copy-Item -Path $from -Destination $to -Recurse
  }
}

Copy-IfMissing $src_config $dst_config settings.toml
Copy-IfMissing $src_config $dst_config keybindings.yaml
Copy-IfMissing $src_data   $dst_data   themes
Copy-IfMissing $src_data   $dst_data   workflows
Copy-IfMissing $src_data   $dst_data   launch_configurations
Copy-IfMissing $src_data   $dst_data   tab_configs
Copy-IfMissing $src_home   $dst_home   .mcp.json
Copy-IfMissing $src_home   $dst_home   skills
```

`[ ! -e ... ]` / `-not (Test-Path $to)` This layer of guard is to avoid overwriting the Waz you have alreadychanged content. If you just want the OpenWarp value to overwrite the Waz, just remove it.
After confirming that everything in Waz is normal, you can delete the OpenWarp directories above to reclaim space. They no longer knowused by any program.
---

## 2. Migrated from upstream Warp
Upstream Warp is another independent product with its own disk identity (see "Upstream Warp source paths" table above).Waz is compiled with channel = `Oss`, corresponding to independent app id (`dev.waz.Waz`) and separated by platformDirectory layout. Both parties cannot see each other's files - this is exactly what Waz allows your Warp account/cloud to doThe reason why the status remains on Warp's side.
The text format file schema in the table below is stable and safe; **Other things are not necessarily guaranteed** —— WarpIndependently evolving, binary/private storage may be tied to Warp's authentication and bundle identity.
### What can be copied
Same 8 items as section 1:
| File/Directory | Category | What to Control ||---|---|---|
| `settings.toml` | config | Public settings (TOML settings file). || `keybindings.yaml` | config | Custom shortcut keys. || `themes/` | data | Custom theme. || `workflows/` | data | Custom workflow. || `launch_configurations/` | data | Launch configurations. || `tab_configs/` | data | Tab configuration. || `.mcp.json` | home dotfile | MCP server configuration. || `skills/` | home dotfile | Agent skills。 |

### **DO NOT** COPY CONTENT
- **`user_preferences.json`** - This is private storage, located on macOS  `~/Library/Application Support/dev.warp.Warp/`(Linux / Windows corresponding state  directory), which is a mix of user preferences, login tokens, machine binding IDs, and cloud cache status. whole file  Copying it will leak identity information and cause Waz to misjudge the login status. Waz default value itself is already privacy  First thing's first, **don't touch it**.- **`warp.sqlite`** (and `-wal` / `-shm` companion files) - schema coupled with upstream Warp,  There is no guarantee that Waz migrations can be run.- **Entry in Keychain/DPAPI/libsecret** - Bundle/service bound to Warp  Name,has no meaning to Waz.
### Operation steps
> **Turn off Warp and Waz** before copying.
**macOS**

```sh
mkdir -p "$HOME/.waz"
for f in settings.toml keybindings.yaml themes workflows launch_configurations tab_configs skills .mcp.json; do
  if [ -e "$HOME/.warp/$f" ] && [ ! -e "$HOME/.waz/$f" ]; then
    cp -R "$HOME/.warp/$f" "$HOME/.waz/$f"
  fi
done
```

**Linux**

```sh
src_config="${XDG_CONFIG_HOME:-$HOME/.config}/warp-terminal"
src_data="${XDG_DATA_HOME:-$HOME/.local/share}/warp-terminal"
src_home="$HOME/.warp"

dst_config="${XDG_CONFIG_HOME:-$HOME/.config}/waz"
dst_data="${XDG_DATA_HOME:-$HOME/.local/share}/waz"
dst_home="$HOME/.waz"
mkdir -p "$dst_config" "$dst_data" "$dst_home"

copy() {
  if [ -e "$1/$3" ] && [ ! -e "$2/$3" ]; then
    cp -R "$1/$3" "$2/$3"
  fi
}

copy "$src_config" "$dst_config" settings.toml
copy "$src_config" "$dst_config" keybindings.yaml
copy "$src_data"   "$dst_data"   themes
copy "$src_data"   "$dst_data"   workflows
copy "$src_data"   "$dst_data"   launch_configurations
copy "$src_data"   "$dst_data"   tab_configs
copy "$src_home"   "$dst_home"   .mcp.json
copy "$src_home"   "$dst_home"   skills
```

**Windows(PowerShell)**

```powershell
$src_config = "$env:LOCALAPPDATA\warp\Warp-Terminal\config"
$src_data   = "$env:APPDATA\warp\Warp-Terminal\data"
$src_home   = "$env:USERPROFILE\.warp"

$dst_config = "$env:LOCALAPPDATA\waz\Waz\config"
$dst_data   = "$env:APPDATA\waz\Waz\data"
$dst_home   = "$env:USERPROFILE\.waz"
New-Item -ItemType Directory -Force -Path $dst_config, $dst_data, $dst_home | Out-Null

function Copy-IfMissing($srcDir, $dstDir, $name) {
  $from = Join-Path $srcDir $name
  $to   = Join-Path $dstDir $name
  if ((Test-Path $from) -and -not (Test-Path $to)) {
    Copy-Item -Path $from -Destination $to -Recurse
  }
}

Copy-IfMissing $src_config $dst_config settings.toml
Copy-IfMissing $src_config $dst_config keybindings.yaml
Copy-IfMissing $src_data   $dst_data   themes
Copy-IfMissing $src_data   $dst_data   workflows
Copy-IfMissing $src_data   $dst_data   launch_configurations
Copy-IfMissing $src_data   $dst_data   tab_configs
Copy-IfMissing $src_home   $dst_home   .mcp.json
Copy-IfMissing $src_home   $dst_home   skills
```

Warp's own data will not be changed from beginning to end, and Warp itself will continue to be available.
---

## verify
Launch Waz, you should see the custom theme in the theme selector and Customize in the shortcut editorKey position, see the custom workflow in the workflow launcher. All items that appear in the settings interfaceThe items in `settings.toml` should have the same value as the source.
If anything is wrong, the problem must be in one of the 8 files above - open it with a text editor and take a look, orDelete it directly and let Waz use the default value.
## rollback
The operations in this article are **not destructive**: every file copied is automatically used by default when Waz starts.value reconstructed. Overall rollback:
```sh
# macOS
rm -rf ~/.waz
```

```sh
# Linux
rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/waz"
rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/waz"
rm -rf "$HOME/.waz"
```

```powershell
# Windows
Remove-Item -Recurse -Force "$env:APPDATA\waz"
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\waz"
Remove-Item -Recurse -Force "$env:USERPROFILE\.waz"
```

Neither OpenWarp nor Warp's source directories will be modified by this guide.
