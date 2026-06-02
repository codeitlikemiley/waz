# Waz へのsettingsのshift
[English](./migrate-from-warp.md) · [Simplified Chinese](./migrate-from-warp.zh-CN.md)
このガイドは, **setting system composition**MCP settingなど)を前のインストールから Wazへ citedき継ぎたいdirectionalけです.Migration element としてscenario されるのは 2 つあり, **両者ではsecurity プロファイルがdifferent なる**ため,This book is written in a different style. The situation where the person should be, the correct situation OpenWarp からpopularしてから** Warp の动行を検椟してください.1. **OpenWarp**——The former name of Waz.2. **上流の[Warp](https://github.com/warpdotdev/warp)**——Waz が fork して いるプロジェクト.
This book is a guide to SQLite documenta schema がcombination したストアにgna されてIt's safe and secure.---

## ディスク上のレイアウトWaz(および OpenWarp / 上流Warp も同様)はディスク上のSTATEを **3 typesのディレクトリ**に分けてgnaします:- **config** —— `settings.toml`、`keybindings.yaml`
- **data** —— `themes/`、`workflows/`、`launch_configurations/`、`tab_configs/`
- **home dotfile** —— `.mcp.json`、`skills/`

macOS では 3 カテゴリーがいずれも単一の home dotfile ディレクトリ(`~/.warp/`, `~/.openwarp/`, または`~/.waz/`)にINTEGRATED されます.Linux ではSave theにconfigurationします.### Waz のSave first
| カテゴリー | macOS | Linux | Windows |
|---|---|---|---|
| config | `~/.waz/` | `${XDG_CONFIG_HOME:-~/.config}/waz/` | `%LOCALAPPDATA%\waz\Waz\config\` |
| data | `~/.waz/` | `${XDG_DATA_HOME:-~/.local/share}/waz/` | `%APPDATA%\waz\Waz\data\` |
| home dotfile | `~/.waz/` | `~/.waz/` | `%USERPROFILE%\.waz\` |

### OpenWarp のソースパス

| カテゴリー | macOS | Linux | Windows |
|---|---|---|---|
| config | `~/.openwarp/` | `${XDG_CONFIG_HOME:-~/.config}/openwarp/` | `%LOCALAPPDATA%\openwarp\OpenWarp\config\` |
| data | `~/.openwarp/` | `${XDG_DATA_HOME:-~/.local/share}/openwarp/` | `%APPDATA%\openwarp\OpenWarp\data\` |
| home dotfile | `~/.openwarp/` | `~/.openwarp/` | `%USERPROFILE%\.openwarp\` |

### Upper class Warp のソースパス
| カテゴリー | macOS | Linux | Windows |
|---|---|---|---|
| config | `~/.warp/` | `${XDG_CONFIG_HOME:-~/.config}/warp-terminal/` | `%LOCALAPPDATA%\warp\Warp-Terminal\config\` |
| data | `~/.warp/` | `${XDG_DATA_HOME:-~/.local/share}/warp-terminal/` | `%APPDATA%\warp\Warp-Terminal\data\` |
| home dotfile | `~/.warp/` | `~/.warp/` | `%USERPROFILE%\.warp\` |

> On Windows> organizationフォルダname はパッケージ method によってdifferent なる occasion があります.> `%APPDATA%\warp\Warp-Terminal`(または`%LOCALAPPDATA%\warp\Warp-Terminal`)> に见つからないoccasionは、お使いの Warp が実记に用している> `%APPDATA%` / `%LOCALAPPDATA%`のパスをconfirmationしてください.---

## 1. OpenWarp migration (existing ユーザーのTui娨経路)OpenWarp は前のWaz そのものです. Changed name to Komit(`feat: rename project Warp/OpenWarp → Waz`)はidentifierとディスク上のパス名をChanging update しただけで, **setting The next note isのファイルはそのままコピーできます.### コピー対shaw| ファイル / ディレクトリ | カテゴリー | 丶 cut ||---|---|---|| `settings.toml` | config | Public settings (TOML ベースのSETファイル). || `keybindings.yaml` | config | カスタムキーバインド。 |
| `themes/` | data | カスタムテーマ。 |
| `workflows/` | data | カスタムワークフロー。 |
| `launch_configurations/` | data | Launch settings. || `tab_configs/` | data | Tab settings. || `.mcp.json` | home dotfile | MCP settings. || `skills/` | home dotfile | Agent skills。 |

### Good luck
> コピー前に **Waz をEnd**してください.プロセスがファイルを洴んでいると> Failure.**macOS**

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

`[ ! -e ... ]` / `-not (Test-Path $to)`したcontentを上书きしないためのものです. OpenWarpの値で上书きしたいoccasionは外してください.
Waz がquestion なく动くことをconfirm したら、上记の OpenWarp ディレクトリを出してThe field of ディスク is recycling.もうWhoっていません.---

## 2. Upstream Warp からの migrate
Upstream Warp は independent し た farewell プ ロ ダ ク ト で, independent のディスク上 アイデン ティティをhold ちます(上の「UPstream Warpのソースパス　tableをreference). Waz はビルド时の channel が`Oss` で、 Duoduの app id (`dev.waz.Waz`) Mutual mutual support のファイルを见ることができません——これは Warp のアカウントやクラウドSTATEを Wazにholdち込まないための世様でもあります.The following のテキスト form ファイルは schema is stable and stable, safe and secure.**それ外はそうではありません** —— Warp は Waz とは independent にevolutionしており,バイナリ / プライベートストアは Warp のcertificationや bundle identification subにNew pay いているOccasion があります.### コピー対shawセクション 1 と同じ 8 items です:| ファイル / ディレクトリ | カテゴリー | 丶 cut ||---|---|---|| `settings.toml` | config | Public settings (TOML ベースのSETファイル). || `keybindings.yaml` | config | カスタムキーバインド。 |
| `themes/` | data | カスタムテーマ。 |
| `workflows/` | data | カスタムワークフロー。 |
| `launch_configurations/` | data | Launch settings. || `tab_configs/` | data | Tab settings. || `.mcp.json` | home dotfile | MCP settings. || `skills/` | home dotfile | Agent skills。 |

### コピー**してはいけない**もの

- **`user_preferences.json`** —— macOS の
  `~/Library/Application Support/dev.warp.Warp/`(Linux / Windows ではそれに
  Quite する state ディレクトリ)にあるプライベートストアで、ユーザーsetting・ Authentication トークン・マシンdepends on ID・クラウドキャッシュがmixed in しています.ファイル ごとコピーすると ID information が乐れたり, Waz のcertification statusが壊れたりします. Wazのデフォルトはもともとプライバシーpriorityなので、**touchらないでください**.- **`warp.sqlite`**(および`-wal` / `-shm` のサイドカー)—— schema が上上WARp にcombination しており, Waz のmigration が通るguarantee はありません.- **Keychain / DPAPI / libsecret のエントリ** —— Warp の bundle / service name に New pay いており, Waz からは utilizing できません.### Good luck
> コピー前に **Warp と Waz をEnd**してください.**macOS**

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

Warp 自体のデータはAll hands を加えません —— Warp original body はそのまま使い続けられます.---

## Confirmation method
Waz Starterカスタムキーバインド、ワークフローランチャーにカスタムワークフローが见えるはずです. `settings.toml` exists and is reflected in the settings UI.されます．
うまく reflects the されない occasion and reason. 8 ファイルのいずれかに有まれています——テキストエディタで开いてConfirmationするか、Deletionして Wazのデフォルトに戻してください.
## ロールバック

The operation of this book is very simple. When activated, the にデフォルトから regenerates intoできるものです.すべてを元に戻すには:```sh
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

OpenWarp Warp
