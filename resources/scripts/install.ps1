# Installs or updates GlazeWM on Windows from the latest GitHub release.
#
# The installer is unsigned. Downloading via PowerShell doesn't add the
# Mark of the Web, so SmartScreen doesn't block the installer.
#
# Usage: irm https://raw.githubusercontent.com/SigmaGrindset/glazewm/main/resources/scripts/install.ps1 | iex

# Wrapped in a script block, so that variables and preferences don't leak
# into the caller's session when run via `iex`.
& {
  $ErrorActionPreference = 'Stop'

  # Progress bar significantly slows down `Invoke-WebRequest` in Windows
  # PowerShell.
  $ProgressPreference = 'SilentlyContinue'

  $repo = if ($env:GLAZEWM_REPO) { $env:GLAZEWM_REPO } else { 'SigmaGrindset/glazewm' }
  $registryKey = 'HKLM:\SOFTWARE\glzr.io\GlazeWM'

  # Returns the install directory written by the MSI, or `$null` if GlazeWM
  # isn't installed.
  function Get-InstallDir() {
    $entry = Get-ItemProperty -Path $registryKey -Name 'InstallDir' -ErrorAction SilentlyContinue
    if ($entry) { $entry.InstallDir } else { $null }
  }

  # Returns the IDs of running WM processes.
  #
  # CLI processes (e.g. `glazewm sub`) share the same process name. The path
  # of an elevated WM process can't be read, so filter by exclusion instead.
  # Exited processes stay listed (without threads) while other processes hold
  # handles to them, so those are excluded as well.
  function Get-WmProcessIds([string]$cliExe) {
    Get-Process -Name 'glazewm' -ErrorAction SilentlyContinue |
      Where-Object { $_.Threads.Count -gt 0 -and $_.Path -ne $cliExe } |
      Select-Object -ExpandProperty Id
  }

  $arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'x64' }
    'ARM64' { 'arm64' }
    default { throw "Unsupported architecture '$env:PROCESSOR_ARCHITECTURE'." }
  }

  $msiName = "glazewm-windows-$arch.msi"
  $msiUrl = "https://github.com/$repo/releases/latest/download/$msiName"
  $msiPath = Join-Path ([System.IO.Path]::GetTempPath()) $msiName

  try {
    Write-Host "Downloading $msiUrl..."
    Invoke-WebRequest -Uri $msiUrl -OutFile $msiPath -UseBasicParsing

    # Gracefully exit the running instance, so that it runs its shutdown
    # cleanup and its files can be replaced.
    $installDir = Get-InstallDir
    if ($installDir) {
      $cliExe = Join-Path $installDir 'cli\glazewm.exe'

      if (@(Get-WmProcessIds $cliExe).Count -gt 0) {
        Write-Host 'Exiting running GlazeWM instance...'
        & $cliExe command wm-exit | Out-Null

        $deadline = (Get-Date).AddSeconds(10)
        while (@(Get-WmProcessIds $cliExe).Count -gt 0 -and (Get-Date) -lt $deadline) {
          Start-Sleep -Milliseconds 500
        }

        if (@(Get-WmProcessIds $cliExe).Count -gt 0) {
          throw 'GlazeWM is still running. Exit it manually and re-run this script.'
        }
      }
    }

    # `/passive` skips the installer dialogs and only shows a progress bar.
    Write-Host 'Installing GlazeWM...'
    $installer = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', "`"$msiPath`"", '/passive') -Wait -PassThru

    # Exit code 3010 indicates success, but with a pending reboot.
    if ($installer.ExitCode -notin @(0, 3010)) {
      throw "Installer failed with exit code $($installer.ExitCode)."
    }

    $installDir = Get-InstallDir
    if (!$installDir) {
      throw 'Failed to resolve the install directory after installing.'
    }

    Write-Host 'Launching GlazeWM...'
    Start-Process -FilePath (Join-Path $installDir 'glazewm.exe')

    Write-Host 'Done. Restart open terminals to use the `glazewm` CLI.'
  }
  finally {
    Remove-Item -Path $msiPath -Force -ErrorAction SilentlyContinue
  }
}
