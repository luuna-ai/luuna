# Fixed local development service bootstrap. EmitConnection is for the native private pipe only.
[CmdletBinding()]
param([string]$DataDirectory, [switch]$EmitConnection)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
. (Join-Path $PSScriptRoot 'windows-common.ps1')
$paths = Get-LuunaWindowsPaths
$root = Split-Path -Parent $PSScriptRoot
# A packaged installation (scripts/package-windows.ps1) places the runtime and the
# pinned Pi tree beside this helper; a development checkout uses its build outputs.
$installed = Test-Path -LiteralPath (Join-Path $root 'luuna-runtime.exe')
$runtime = if ($installed) { Join-Path $root 'luuna-runtime.exe' } else { Join-Path $root 'target\debug\luuna-runtime.exe' }
$connectionFile = Join-Path $paths.Credentials 'connection.json'
$mutex = New-Object Threading.Mutex($false, ('Local\LuunaRuntime-' + [Security.Principal.WindowsIdentity]::GetCurrent().User.Value))
$locked = $false
$token = $null
try {
    try { $locked = $mutex.WaitOne(30000) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (!$locked) { throw 'Another Luuna startup is still running. Retry shortly.' }
    if (!(Test-Path -LiteralPath $runtime)) { throw 'Build the Windows runtime first; see docs/WINDOWS-VERIFICATION.md.' }
    $previous = $null
    if (Test-Path -LiteralPath $connectionFile) { $previous = Get-Content -LiteralPath $connectionFile -Raw | ConvertFrom-Json }
    if (!$DataDirectory) {
        $DataDirectory = if ($previous -and $previous.data_directory) { $previous.data_directory } else { Join-Path $paths.Home 'windows' }
    }
    $DataDirectory = [IO.Path]::GetFullPath($DataDirectory)
    if ($previous) {
        if ($previous.runtime_url -ne 'http://127.0.0.1:4242') { throw 'Unexpected saved runtime endpoint.' }
        $saved = ConvertTo-SecureString $previous.encrypted_token
        $pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($saved)
        try { $token = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer) }
        finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer); $saved.Dispose() }
        if ($token -notmatch '^[a-f0-9]{64}$') { throw 'Invalid saved runtime token.' }
    } else {
        $bytes = New-Object byte[] 32
        $random = [Security.Cryptography.RandomNumberGenerator]::Create()
        try { $random.GetBytes($bytes) } finally { $random.Dispose() }
        $token = [BitConverter]::ToString($bytes).Replace('-','').ToLowerInvariant()
    }
    $headers = @{ Authorization = 'Bearer ' + $token }
    function Save-Connection([int]$RuntimeId) {
        $connection = @{ runtime_pid = $RuntimeId; runtime_path = $runtime; data_directory = $DataDirectory; runtime_url = 'http://127.0.0.1:4242'; encrypted_token = (ConvertFrom-SecureString (ConvertTo-SecureString $token -AsPlainText -Force)) }
        if ($previous) {
            foreach ($field in @('client_pid','native','client_url')) { if ($previous.PSObject.Properties.Name -contains $field) { $connection[$field] = $previous.$field } }
        }
        $temporary = $connectionFile + '.' + $PID + '.tmp'
        $connection | ConvertTo-Json | Set-Content -LiteralPath $temporary
        Move-Item -LiteralPath $temporary -Destination $connectionFile -Force
    }
    $listener = @(Get-NetTCPConnection -State Listen -LocalPort 4242 -ErrorAction SilentlyContinue)
    $service = $null
    if ($listener.Count) {
        $owners = @($listener.OwningProcess | Select-Object -Unique)
        if (!$previous -or $owners.Count -ne 1) { throw 'Port 4242 belongs to an unrecognized service.' }
        $service = Get-Process -Id $owners[0] -ErrorAction Stop
        if ($service.Path -ne $runtime) { throw 'Port 4242 belongs to another executable.' }
        if (![string]::Equals($DataDirectory, [IO.Path]::GetFullPath($previous.data_directory), [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Stop the existing runtime explicitly before switching data directories.'
        }
        # Also authenticates a spawn whose caller exited before saving its PID.
        Invoke-RestMethod 'http://127.0.0.1:4242/api/v1/runs' -Headers $headers -MaximumRedirection 0 -TimeoutSec 5 | Out-Null
    } else {
        # Never race an owned process still starting or exiting, even before it binds its port.
        if ($previous -and $previous.runtime_pid) {
            $starting = Get-Process -Id $previous.runtime_pid -ErrorAction SilentlyContinue
            if ($starting -and $starting.Path -eq $runtime -and !$starting.WaitForExit(5000)) {
                throw 'The recorded runtime is not accepting connections yet. Retry shortly; no duplicate was started.'
            }
        }
        if ($previous -and !$previous.runtime_pid -and (Get-Process luuna-runtime -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $runtime })) {
            throw 'An uncertain runtime startup is still running. Retry shortly; no duplicate was started.'
        }
        $providerFile = Join-Path $paths.Credentials 'provider.json'
        if (!(Test-Path -LiteralPath $providerFile)) { throw 'Save your provider key with Configure Luuna.cmd first.' }
        $record = Get-Content -LiteralPath $providerFile -Raw | ConvertFrom-Json
        if ($record.provider -notin @('openrouter','openai','anthropic')) { throw 'Unsupported saved provider.' }
        $node = (Get-Command node.exe -ErrorAction Stop).Source
        $pi = if ($installed) { Join-Path $root 'pi\node_modules\@earendil-works\pi-coding-agent\dist\cli.js' } else { Join-Path $root 'tools\pi\node_modules\@earendil-works\pi-coding-agent\dist\cli.js' }
        if (!(Test-Path -LiteralPath $pi)) { throw 'Install the pinned tools/pi dependencies first.' }
        $keyName = 'LUUNA_PI_' + $record.provider.ToUpperInvariant() + '_API_KEY'
        $settings = @{}
        $names = @($keyName,'LUUNA_API_TOKEN','LUUNA_DATA_DIR','LUUNA_PORT','LUUNA_PI_COMMAND','LUUNA_PI_ARGS_JSON','LUUNA_PI_PROVIDER','LUUNA_PI_MODEL')
        foreach ($name in $names) { $settings[$name] = [Environment]::GetEnvironmentVariable($name,'Process') }
        $secret = $null
        $pointer = [IntPtr]::Zero
        try {
            $secret = ConvertTo-SecureString $record.encrypted_key
            $pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secret)
            [Environment]::SetEnvironmentVariable($keyName,[Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer),'Process')
            $env:LUUNA_API_TOKEN = $token
            $env:LUUNA_DATA_DIR = $DataDirectory
            $env:LUUNA_PORT = '4242'
            $env:LUUNA_PI_COMMAND = $node
            $env:LUUNA_PI_ARGS_JSON = ConvertTo-Json -InputObject @($pi) -Compress
            $env:LUUNA_PI_PROVIDER = $record.provider
            $env:LUUNA_PI_MODEL = switch ($record.provider) { 'openrouter' {'openai/gpt-4.1-mini'} 'openai' {'gpt-4.1-mini'} default {'claude-sonnet-4-5'} }
            # Persist the encrypted identity first; authenticate/adopt after an uncertain spawn.
            Save-Connection 0
            $service = Start-Process -FilePath $runtime -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardError (Join-Path $paths.Logs 'runtime.log') -RedirectStandardOutput (Join-Path $paths.Logs 'runtime-stdout.log') -PassThru
        } finally {
            foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name,$settings[$name],'Process') }
            if ($pointer -ne [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer) }
            if ($secret) { $secret.Dispose() }
        }
    }
    # Save identity before waiting so a retry can reconcile an uncertain startup.
    Save-Connection $service.Id
    $ready = $false
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($service.HasExited) { throw 'The runtime exited during startup. Check your .luuna/logs folder.' }
        try {
            $health = Invoke-RestMethod 'http://127.0.0.1:4242/health' -MaximumRedirection 0 -TimeoutSec 1
            Invoke-RestMethod 'http://127.0.0.1:4242/api/v1/runs' -Headers $headers -MaximumRedirection 0 -TimeoutSec 1 | Out-Null
            if ($health.status -eq 'ok') { $ready = $true; break }
        } catch { Start-Sleep -Milliseconds 250 }
    }
    if (!$ready) { throw 'The runtime did not become ready. Check your .luuna/logs folder.' }
    $result = @{ url = 'http://127.0.0.1:4242'; token = $token }
    if ($EmitConnection) { $result | ConvertTo-Json -Compress } else { return $result }
} finally {
    $token = $null
    $headers = $null
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
