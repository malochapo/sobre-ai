<#
.SYNOPSIS
Installs sobre from a GitHub release, on Windows (PowerShell 5.1 or later).

.DESCRIPTION
    irm https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.ps1 | iex

With options:

    & ([scriptblock]::Create((irm https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.ps1))) -Dense

It downloads the archive and SHA256SUMS from the release, refuses an archive
whose checksum does not match, copies sobre.exe into
%LOCALAPPDATA%\Programs\sobre (or -Dir) and adds that folder to the user PATH.
It runs only `sobre --version` once installed.

.PARAMETER Version
The release to install, e.g. v0.1.0-rc.1. Default: the latest release, or the
newest pre-release while there is no release yet.

.PARAMETER Dense
Install the build with local embeddings.

.PARAMETER Dir
Where to put sobre.exe. Default: %LOCALAPPDATA%\Programs\sobre.
#>
param(
    [string] $Version = '',
    [switch] $Dense,
    [string] $Dir = ''
)

function Install-Sobre {
    param([string] $Version, [switch] $Dense, [string] $Dir)

    $ErrorActionPreference = 'Stop'
    # The progress bar slows Invoke-WebRequest down considerably on PowerShell 5.1.
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'malochapo/sobre-ai'
    $githubUrl = 'https://github.com'
    $apiUrl = 'https://api.github.com'

    if ($PSVersionTable.PSEdition -eq 'Core' -and -not $IsWindows) {
        throw 'this script installs sobre on Windows; on Linux and macOS, use install.sh'
    }
    # A 32-bit PowerShell on 64-bit Windows reports the real architecture here.
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64') {
        throw "there is no sobre build for Windows on $arch, only for x64"
    }
    if ($Dense) {
        $asset = 'sobre-dense-x86_64-pc-windows-msvc.zip'
    } else {
        $asset = 'sobre-x86_64-pc-windows-msvc.zip'
    }

    # GitHub accepts TLS 1.2 or later only, which an older .NET does not offer by default.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    if ($Version) {
        if (-not $Version.StartsWith('v')) { $Version = "v$Version" }
        if ($Version -notmatch '^[A-Za-z0-9._-]+$') { throw "'$Version' is not a release tag" }
    } else {
        $Version = Get-SobreReleaseTag -ApiUrl $apiUrl -GitHubUrl $githubUrl -Repo $repo
    }

    $base = "$githubUrl/$repo/releases/download/$Version"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('sobre-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "Installing sobre $Version ($asset)"
        $sums = Join-Path $tmp 'SHA256SUMS'
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums
        } catch {
            throw "could not download $base/SHA256SUMS: is $Version a sobre release? ($($_.Exception.Message))"
        }
        $zip = Join-Path $tmp $asset
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $zip
        } catch {
            throw "could not download $base/$asset ($($_.Exception.Message))"
        }

        $expected = $null
        foreach ($line in (Get-Content -LiteralPath $sums)) {
            $parts = $line.Trim() -split '\s+', 2
            if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq $asset) {
                $expected = $parts[0].ToLowerInvariant()
                break
            }
        }
        if (-not $expected) { throw "SHA256SUMS of $Version lists no $asset" }
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "checksum mismatch for $asset (expected $expected, got $actual): nothing was installed"
        }

        $extract = Join-Path $tmp 'extract'
        Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
        $exe = Join-Path $extract 'sobre.exe'
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "$asset holds no sobre.exe" }

        if (-not $Dir) { $Dir = Join-Path $env:LOCALAPPDATA 'Programs\sobre' }
        New-Item -ItemType Directory -Force -Path $Dir | Out-Null
        $Dir = (Resolve-Path -LiteralPath $Dir).ProviderPath.TrimEnd('\')
        $dest = Join-Path $Dir 'sobre.exe'

        # A running sobre.exe (an agent's MCP server) cannot be overwritten or
        # deleted, but it can be renamed: the old binary is moved aside, and
        # deleted when nothing runs it any more.
        Remove-SobreLeftovers -Dir $Dir
        $aside = $null
        if (Test-Path -LiteralPath $dest) {
            $aside = Join-Path $Dir ('sobre.exe.old-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
            Move-Item -LiteralPath $dest -Destination $aside
        }
        try {
            Copy-Item -LiteralPath $exe -Destination $dest
        } catch {
            if ($aside) { Move-Item -LiteralPath $aside -Destination $dest }
            throw "could not write $dest ($($_.Exception.Message))"
        }
        Remove-SobreLeftovers -Dir $Dir
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    try {
        $installed = & $dest --version
    } catch {
        throw "installed $dest, but it does not run: $($_.Exception.Message)"
    }
    if ($LASTEXITCODE -ne 0) { throw "installed $dest, but it does not run (exit code $LASTEXITCODE)" }
    Write-Host "Installed $installed at $dest"

    if (Add-SobreToUserPath -Dir $Dir) {
        Write-Host ''
        Write-Host "Added $Dir to your user PATH: terminals opened from now on find sobre."
    }
    # This session finds sobre too.
    if (-not $env:Path) {
        $env:Path = $Dir
    } elseif (-not (Test-PathListHas -List $env:Path -Dir $Dir)) {
        $env:Path = $env:Path.TrimEnd(';') + ";$Dir"
    }
    $found = Get-Command sobre -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($found -and $found.Path -ne $dest) {
        Write-Host ''
        Write-Host "Note: $($found.Path) comes before $dest on your PATH."
    }

    Write-Host ''
    Write-Host "Next: run 'sobre init' to connect your agents, then restart them."
}

# The tag of the latest release or, while there is none, of the newest
# pre-release. Listing the releases also covers a failed first request: the
# newest stable release in the list wins.
function Get-SobreReleaseTag {
    param([string] $ApiUrl, [string] $GitHubUrl, [string] $Repo)

    try {
        $latest = Invoke-RestMethod -UseBasicParsing -Uri "$ApiUrl/repos/$Repo/releases/latest"
        if ($latest.tag_name) { return [string] $latest.tag_name }
    } catch {
        # No stable release yet (404), or a failed request: the list decides.
    }
    try {
        $releases = Invoke-RestMethod -UseBasicParsing -Uri "$ApiUrl/repos/$Repo/releases?per_page=100"
    } catch {
        throw "could not list the releases of $GitHubUrl/$Repo (GitHub API unreachable or rate-limited); pass -Version <tag>"
    }
    $first = $null
    foreach ($release in $releases) {
        if (-not $first) { $first = $release }
        if (-not $release.prerelease) { return [string] $release.tag_name }
    }
    if ($first) { return [string] $first.tag_name }
    throw "no sobre release found at $GitHubUrl/$Repo/releases"
}

# Deletes the binaries an earlier install moved aside; one still running stays.
function Remove-SobreLeftovers {
    param([string] $Dir)
    Get-ChildItem -LiteralPath $Dir -Filter 'sobre.exe.old-*' -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
}

function Test-PathListHas {
    param([string] $List, [string] $Dir)
    foreach ($entry in ($List -split ';')) {
        if (-not $entry) { continue }
        $expanded = [Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\')
        if ($expanded -ieq $Dir) { return $true }
    }
    return $false
}

# Appends $Dir to the user PATH in the registry, keeping the value's type and
# any %VARIABLE% it holds, then tells running programs the environment
# changed. Returns whether it added the entry.
function Add-SobreToUserPath {
    param([string] $Dir)
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        $current = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if (Test-PathListHas -List $current -Dir $Dir) { return $false }
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        if ($null -ne $key.GetValue('Path')) { $kind = $key.GetValueKind('Path') }
        if ($current) {
            $value = $current.TrimEnd(';') + ";$Dir"
        } else {
            $value = $Dir
        }
        $key.SetValue('Path', $value, $kind)
    } finally {
        $key.Close()
    }
    try {
        if (-not ('SobreInstall.NativeMethods' -as [type])) {
            Add-Type -Namespace SobreInstall -Name NativeMethods -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam,
    string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG, 5 seconds.
        [void] [SobreInstall.NativeMethods]::SendMessageTimeout([IntPtr] 0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref] $result)
    } catch {
        Write-Host 'Note: sign out and back in if a new terminal does not find sobre.'
    }
    return $true
}

# Errors end the install with a message. `exit` would also close the
# PowerShell window that ran `irm ... | iex`, so it is only used when the
# script runs from a file.
$sobreRunFromFile = $MyInvocation.MyCommand.CommandType -eq 'ExternalScript'
try {
    Install-Sobre -Version $Version -Dense:$Dense -Dir $Dir
} catch {
    Write-Host "sobre install: $($_.Exception.Message)" -ForegroundColor Red
    if ($sobreRunFromFile) { exit 1 }
}
