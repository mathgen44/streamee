<#
.SYNOPSIS
  Installe le filtre audio Streamee sur un micro : les sons de Streamee sortent alors
  directement par ce micro, dans tous les jeux et logiciels, sans micro virtuel.

.DESCRIPTION
  1. Copie StreameeApo.dll dans Program Files\Streamee.
  2. Déclare l'APO auprès de Windows (COM + AudioEngine\AudioProcessingObjects).
  3. Crée le tampon partagé C:\ProgramData\Streamee\inject.shm.
  4. Pose l'APO sur le micro choisi (FxProperties), en sauvegardant les réglages d'origine
     dans HKLM\SOFTWARE\Streamee\Backup pour que uninstall-apo.ps1 puisse tout remettre.
  5. Redémarre le service audio de Windows.

.PARAMETER DllPath
  Chemin de StreameeApo.dll (par défaut : à côté de ce script).

.PARAMETER EndpointId
  GUID du micro ({xxxxxxxx-...}). Sans ce paramètre, le script propose la liste des micros.

.PARAMETER FromApp
  Lancé par l'application : fermeture automatique si tout va bien, pause sur erreur.
#>
#Requires -RunAsAdministrator
[CmdletBinding()]
param(
    [string]$DllPath,
    [string]$EndpointId,
    [switch]$FromApp
)

$ErrorActionPreference = 'Stop'

# Journal, utile si la fenêtre se ferme trop vite : %TEMP%\streamee-install.log
try { Start-Transcript -Path (Join-Path $env:TEMP 'streamee-install.log') -Force | Out-Null } catch { }

# En cas d'erreur, la fenêtre reste ouverte pour qu'on puisse la lire (lancé depuis Streamee).
# Le trap doit être au niveau du script : placé dans un bloc if, il ne s'appliquerait qu'à ce bloc.
trap {
    Write-Host ''
    Write-Host "Erreur : $_" -ForegroundColor Red
    Write-Host $_.InvocationInfo.PositionMessage -ForegroundColor DarkGray
    try { Stop-Transcript | Out-Null } catch { }
    if ($FromApp) { Read-Host 'Appuie sur Entrée pour fermer' }
    exit 1
}

# $PSScriptRoot n'est pas encore défini dans les valeurs par défaut de param() sous PowerShell 5.1.
# Le chemin peut arriver au format long « \\?\D:\... », que Split-Path/Join-Path ne gèrent pas : on passe par .NET.
if (-not $DllPath) { $DllPath = [System.IO.Path]::Combine([System.IO.Path]::GetDirectoryName($MyInvocation.MyCommand.Path), 'StreameeApo.dll') }
if ($DllPath.StartsWith('\\?\')) { $DllPath = $DllPath.Substring(4) }

$Clsid        = '{EE91D2D7-78FB-42EB-9BB7-3190B21658D2}'
$InstallDir   = Join-Path $env:ProgramFiles 'Streamee'
$DataDir      = Join-Path $env:ProgramData 'Streamee'
$ShmPath      = Join-Path $DataDir 'inject.shm'
$ShmBytes     = 4096 + 65536 * 2 * 4
$CaptureRoot  = 'SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture'
$BackupRoot   = 'SOFTWARE\Streamee\Backup'

# Clés de propriétés audio (PKEY) telles que Windows les range dans le registre.
$PK_SFX          = '{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5'   # PKEY_FX_StreamEffectClsid
$PK_SFX_COMPOSITE= '{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},13'  # PKEY_CompositeFX_StreamEffectClsid
$PK_SFX_MODES    = '{d3993a3f-99c2-4402-b5ec-a92a0367664b},5'   # PKEY_SFX_ProcessingModes_Supported_For_Streaming
$PK_DISABLE_FX   = '{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5'   # PKEY_AudioEndpoint_Disable_SysFx
$PK_DEVICE_DESC  = '{a45c254e-df1c-4efd-8020-67d146a850e0},2'   # « Microphone »
$PK_IFACE_NAME   = '{b3f8fa53-0004-438e-9003-51a46e139bfc},6'   # « Arctis Nova Pro », etc.
$MODE_DEFAULT    = '{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}'     # AUDIO_SIGNALPROCESSINGMODE_DEFAULT
$MODE_COMMS      = '{98951333-B9CD-48B1-A0A3-FF40682D73F7}'     # AUDIO_SIGNALPROCESSINGMODE_COMMUNICATIONS (Discord, appels)

function Write-Step($text) { Write-Host "-> $text" -ForegroundColor Cyan }

# ------------------------------------------------------------------ Privilèges
# Les clés des périphériques audio appartiennent au système : pour les modifier il faut
# en prendre possession (comme le fait Equalizer APO).
Add-Type -Language CSharp -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class StreameePriv {
    [StructLayout(LayoutKind.Sequential, Pack = 1)]
    struct TokPriv1Luid { public int Count; public long Luid; public int Attr; }
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr h, int acc, out IntPtr tok);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool LookupPrivilegeValue(string host, string name, out long luid);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool AdjustTokenPrivileges(IntPtr tok, bool disall, ref TokPriv1Luid newst, int len, IntPtr prev, IntPtr relen);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    public static void Enable(string name) {
        IntPtr tok;
        if (!OpenProcessToken(GetCurrentProcess(), 0x28, out tok)) throw new System.ComponentModel.Win32Exception();
        TokPriv1Luid tp; tp.Count = 1; tp.Attr = 2;
        if (!LookupPrivilegeValue(null, name, out tp.Luid)) throw new System.ComponentModel.Win32Exception();
        if (!AdjustTokenPrivileges(tok, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero)) throw new System.ComponentModel.Win32Exception();
    }
}
'@
[StreameePriv]::Enable('SeTakeOwnershipPrivilege')
[StreameePriv]::Enable('SeRestorePrivilege')

$Admins = New-Object System.Security.Principal.SecurityIdentifier('S-1-5-32-544')
$HKLM = [Microsoft.Win32.Registry]::LocalMachine

function Grant-AdminWrite([string]$subKey) {
    $key = $HKLM.OpenSubKey($subKey, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
        [System.Security.AccessControl.RegistryRights]::TakeOwnership)
    if (-not $key) { return }
    $acl = $key.GetAccessControl([System.Security.AccessControl.AccessControlSections]::None)
    $acl.SetOwner($Admins)
    $key.SetAccessControl($acl)
    $key.Close()
    $key = $HKLM.OpenSubKey($subKey, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
        [System.Security.AccessControl.RegistryRights]::ChangePermissions)
    $acl = $key.GetAccessControl()
    $rule = New-Object System.Security.AccessControl.RegistryAccessRule($Admins, 'FullControl',
        'ContainerInherit', 'None', 'Allow')
    $acl.AddAccessRule($rule)
    $key.SetAccessControl($acl)
    $key.Close()
}

# ------------------------------------------------------------------ Choix du micro
function Get-CaptureEndpoints {
    $root = $HKLM.OpenSubKey($CaptureRoot)
    foreach ($id in $root.GetSubKeyNames()) {
        $ep = $root.OpenSubKey($id)
        if ($ep.GetValue('DeviceState') -ne 1) { continue }  # 1 = actif
        $props = $ep.OpenSubKey('Properties')
        $desc = $props.GetValue($PK_DEVICE_DESC)
        $iface = $props.GetValue($PK_IFACE_NAME)
        [pscustomobject]@{ Id = $id; Name = if ($iface) { "$desc ($iface)" } else { "$desc" } }
    }
}

$endpoints = @(Get-CaptureEndpoints)
if ($EndpointId) {
    $target = $endpoints | Where-Object { $_.Id -eq $EndpointId }
    if (-not $target) { throw "Micro $EndpointId introuvable ou inactif." }
} else {
    if ($endpoints.Count -eq 0) { throw 'Aucun micro actif trouvé.' }
    Write-Host ''
    Write-Host 'Sur quel micro Streamee doit-il envoyer les sons ?' -ForegroundColor Yellow
    for ($i = 0; $i -lt $endpoints.Count; $i++) { Write-Host ("  {0}. {1}" -f ($i + 1), $endpoints[$i].Name) }
    do { $choice = Read-Host 'Numéro' } until ($choice -match '^\d+$' -and [int]$choice -ge 1 -and [int]$choice -le $endpoints.Count)
    $target = $endpoints[[int]$choice - 1]
}
Write-Host "Micro choisi : $($target.Name)" -ForegroundColor Green

# ------------------------------------------------------------------ DLL
if (-not (Test-Path $DllPath)) { throw "StreameeApo.dll introuvable : $DllPath" }
Write-Step 'Arrêt du service audio'
Stop-Service -Name audiosrv -Force
try {
    Write-Step "Copie de la DLL dans $InstallDir"
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $installedDll = Join-Path $InstallDir 'StreameeApo.dll'
    Copy-Item -Force $DllPath $installedDll

    Write-Step 'Déclaration de l''APO'
    $clsidKey = $HKLM.CreateSubKey("SOFTWARE\Classes\CLSID\$Clsid")
    $clsidKey.SetValue('', 'Streamee APO')
    $inproc = $clsidKey.CreateSubKey('InprocServer32')
    $inproc.SetValue('', $installedDll)
    $inproc.SetValue('ThreadingModel', 'Both')
    $inproc.Close(); $clsidKey.Close()

    $apoKey = $HKLM.CreateSubKey("SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\$Clsid")
    $dw = [Microsoft.Win32.RegistryValueKind]::DWord
    $apoKey.SetValue('FriendlyName', 'Streamee - injection micro')
    $apoKey.SetValue('Copyright', 'MIT - mathgen44')
    $apoKey.SetValue('MajorVersion', 1, $dw)
    $apoKey.SetValue('MinorVersion', 0, $dw)
    $apoKey.SetValue('Flags', 0x0e, $dw)                 # APO_FLAG_DEFAULT
    $apoKey.SetValue('MinInputConnections', 1, $dw)
    $apoKey.SetValue('MaxInputConnections', 1, $dw)
    $apoKey.SetValue('MinOutputConnections', 1, $dw)
    $apoKey.SetValue('MaxOutputConnections', 1, $dw)
    $apoKey.SetValue('MaxInstances', [int]-1, $dw)       # 0xffffffff
    $apoKey.SetValue('NumAPOInterfaces', 1, $dw)
    $apoKey.SetValue('APOInterface0', '{FD7F2B29-24D0-4B5C-B177-592C39F9CA10}')  # IID_IAudioProcessingObject
    $apoKey.Close()

    # -------------------------------------------------------------- Tampon partagé
    Write-Step "Tampon partagé $ShmPath"
    New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
    # LOCAL SERVICE (audiodg.exe) et utilisateurs authentifiés (Streamee) : lecture/écriture.
    & icacls $DataDir /grant '*S-1-5-19:(OI)(CI)M' '*S-1-5-11:(OI)(CI)M' | Out-Null
    $fs = [System.IO.File]::Open($ShmPath, 'OpenOrCreate', 'ReadWrite', 'ReadWrite')
    try {
        if ($fs.Length -lt $ShmBytes) { $fs.SetLength($ShmBytes) }
        $header = New-Object byte[] 40
        [BitConverter]::GetBytes([uint32]0x4D525453).CopyTo($header, 0)   # 'STRM'
        [BitConverter]::GetBytes([uint32]1).CopyTo($header, 4)            # version
        $fs.Position = 0
        $fs.Write($header, 0, $header.Length)
    } finally { $fs.Close() }

    # -------------------------------------------------------------- Pose sur le micro
    $epPath = "$CaptureRoot\$($target.Id)"
    Write-Step 'Prise en main des clés du micro'
    Grant-AdminWrite $epPath
    Grant-AdminWrite "$epPath\Properties"
    Grant-AdminWrite "$epPath\FxProperties"

    $ep = $HKLM.OpenSubKey($epPath, $true)
    $ep.CreateSubKey('FxProperties').Close()   # certains micros n'en ont pas encore
    Grant-AdminWrite "$epPath\FxProperties"
    $fx = $HKLM.OpenSubKey("$epPath\FxProperties", $true)
    $props = $HKLM.OpenSubKey("$epPath\Properties", $true)

    # Sauvegarde des valeurs d'origine (une seule fois : une réinstallation ne l'écrase pas).
    $backupPath = "$BackupRoot\$($target.Id)"
    if (-not $HKLM.OpenSubKey($backupPath)) {
        Write-Step 'Sauvegarde des réglages d''origine du micro'
        $bk = $HKLM.CreateSubKey($backupPath)
        $bk.SetValue('Name', $target.Name)
        foreach ($pair in @(@('FX', $fx, $PK_SFX), @('FX', $fx, $PK_SFX_COMPOSITE), @('FX', $fx, $PK_SFX_MODES), @('P', $props, $PK_DISABLE_FX))) {
            $src = $pair[1]; $name = $pair[2]
            $names = $src.GetValueNames()
            if ($names -contains $name) {
                $kind = $src.GetValueKind($name)
                $bk.SetValue("$($pair[0])|$name", $src.GetValue($name, $null, 'DoNotExpandEnvironmentNames'), $kind)
            } else {
                $bk.SetValue("$($pair[0])|$name|absent", 1, [Microsoft.Win32.RegistryValueKind]::DWord)
            }
        }
        $bk.Close()
    }

    Write-Step 'Ajout de Streamee dans la chaîne d''effets du micro'
    if ($fx.GetValueNames() -contains $PK_SFX_COMPOSITE) {
        # Chaîne composite (Windows 10+) : on se place en tête, les effets existants restent actifs.
        $list = @($fx.GetValue($PK_SFX_COMPOSITE))
        if ($list -notcontains $Clsid) {
            $fx.SetValue($PK_SFX_COMPOSITE, [string[]](@($Clsid) + $list), [Microsoft.Win32.RegistryValueKind]::MultiString)
        }
    } else {
        $previous = $fx.GetValue($PK_SFX)
        if ($previous -and $previous -ne $Clsid) {
            Write-Warning "Le micro avait déjà un effet ($previous) : il est remplacé et sera remis par uninstall-apo.ps1."
        }
        $fx.SetValue($PK_SFX, $Clsid, [Microsoft.Win32.RegistryValueKind]::String)
    }
    # Modes de traitement où notre effet s'applique : normal et communications.
    $modes = @()
    if ($fx.GetValueNames() -contains $PK_SFX_MODES) { $modes = @($fx.GetValue($PK_SFX_MODES)) }
    foreach ($m in @($MODE_DEFAULT, $MODE_COMMS)) { if ($modes -notcontains $m) { $modes += $m } }
    $fx.SetValue($PK_SFX_MODES, [string[]]$modes, [Microsoft.Win32.RegistryValueKind]::MultiString)
    # « Améliorations audio » activées, sinon Windows ignore tous les effets du micro.
    $kind = if ($props.GetValueNames() -contains $PK_DISABLE_FX) { $props.GetValueKind($PK_DISABLE_FX) } else { [Microsoft.Win32.RegistryValueKind]::DWord }
    $props.SetValue($PK_DISABLE_FX, 0, $kind)
    $fx.Close(); $props.Close(); $ep.Close()

    $state = $HKLM.CreateSubKey('SOFTWARE\Streamee')
    $state.SetValue('Endpoint', $target.Id)
    $state.SetValue('EndpointName', $target.Name)
    $state.Close()
}
finally {
    Write-Step 'Redémarrage du service audio'
    Start-Service -Name audiosrv
}

Write-Host ''
Write-Host "Streamee est installé sur « $($target.Name) »." -ForegroundColor Green
Write-Host 'Les sons de Streamee sortiront par ce micro dès qu''un logiciel l''utilise (jeu, Discord, OBS...).'
try { Stop-Transcript | Out-Null } catch { }
if ($FromApp) { Start-Sleep -Seconds 3 }
