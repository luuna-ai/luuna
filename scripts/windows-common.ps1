# Stable across Explorer and packaged development terminals. Only encrypted records migrate.
function Get-LuunaWindowsPaths {
    $homeDirectory = Join-Path $env:USERPROFILE '.luuna'
    $credentialsDirectory = Join-Path $homeDirectory 'credentials'
    $logsDirectory = Join-Path $homeDirectory 'logs'
    New-Item -ItemType Directory -Force -Path $credentialsDirectory, $logsDirectory | Out-Null
    foreach ($name in @('provider.json', 'connection.json')) {
        $destination = Join-Path $credentialsDirectory $name
        $legacy = Join-Path $env:LOCALAPPDATA ('Luuna\credentials\' + $name)
        if (!(Test-Path -LiteralPath $destination) -and (Test-Path -LiteralPath $legacy)) {
            Copy-Item -LiteralPath $legacy -Destination $destination
        }
    }
    return @{ Home = $homeDirectory; Credentials = $credentialsDirectory; Logs = $logsDirectory }
}

