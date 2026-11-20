<#
.SYNOPSIS
  Retire le filtre audio Streamee de tous les micros et remet leurs réglages d'origine.
#>
#Requires -RunAsAdministrator
[CmdletBinding()]
param([switch]$KeepData)

$ErrorActionPreference = 'Stop'

$Clsid       = '{EE91D2D7-78FB-42EB-9BB7-3190B21658D2}'
$InstallDir  = Join-Path $env:ProgramFiles 'Streamee'
$DataDir     = Join-Path $env:ProgramData 'Streamee'
$CaptureRoot = 'SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture'
$BackupRoot  = 'SOFTWARE\Streamee\Backup'
$PK_SFX_COMPOSITE = '{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},13'
$HKLM = [Microsoft.Win32.Registry]::LocalMachine

Write-Host '-> Arrêt du service audio' -ForegroundColor Cyan
Stop-Service -Name audiosrv -Force
try {
    $backups = $HKLM.OpenSubKey($BackupRoot)
    if ($backups) {
        foreach ($id in $backups.GetSubKeyNames()) {
            $bk = $backups.OpenSubKey($id)
            Write-Host "-> Restauration de « $($bk.GetValue('Name')) »" -ForegroundColor Cyan
            $fx = $HKLM.OpenSubKey("$CaptureRoot\$id\FxProperties", $true)
            $props = $HKLM.OpenSubKey("$CaptureRoot\$id\Properties", $true)
            if (-not $fx -or -not $props) { Write-Warning "Micro $id absent, ignoré."; continue }
            foreach ($entry in $bk.GetValueNames()) {
                if ($entry -eq 'Name') { continue }
                $parts = $entry.Split('|')
                $dest = if ($parts[0] -eq 'P') { $props } else { $fx }
                $name = $parts[1]
                if ($parts.Count -eq 3) {
                    # La valeur n'existait pas avant Streamee.
                    if ($dest.GetValueNames() -contains $name) { $dest.DeleteValue($name) }
                } else {
                    $dest.SetValue($name, $bk.GetValue($entry, $null, 'DoNotExpandEnvironmentNames'), $bk.GetValueKind($entry))
                }
            }
            # Si la chaîne composite a changé depuis, on retire au moins notre CLSID.
            if ($fx.GetValueNames() -contains $PK_SFX_COMPOSITE) {
                $list = @($fx.GetValue($PK_SFX_COMPOSITE)) | Where-Object { $_ -ne $Clsid }
                $fx.SetValue($PK_SFX_COMPOSITE, [string[]]@($list), [Microsoft.Win32.RegistryValueKind]::MultiString)
            }
            $fx.Close(); $props.Close(); $bk.Close()
        }
        $backups.Close()
    }

    Write-Host '-> Suppression de la déclaration de l''APO' -ForegroundColor Cyan
    foreach ($k in @("SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\$Clsid", "SOFTWARE\Classes\CLSID\$Clsid", 'SOFTWARE\Streamee')) {
        if ($HKLM.OpenSubKey($k)) { $HKLM.DeleteSubKeyTree($k) }
    }
    if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
    if (-not $KeepData -and (Test-Path $DataDir)) { Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue }
}
finally {
    Write-Host '-> Redémarrage du service audio' -ForegroundColor Cyan
    Start-Service -Name audiosrv
}
Write-Host 'Streamee est désinstallé de tous les micros.' -ForegroundColor Green
