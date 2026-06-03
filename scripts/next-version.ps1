param(
    [Parameter(Mandatory = $true)]
    [string]$CurrentVersion,

    [string]$CommitText = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Parse-Version {
    param([string]$Version)

    if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)$') {
        throw "CurrentVersion must be SemVer in X.Y.Z form: $Version"
    }

    [pscustomobject]@{
        Major = [int]$Matches[1]
        Minor = [int]$Matches[2]
        Patch = [int]$Matches[3]
    }
}

function Get-BumpLevel {
    param([string]$Text)

    if ([string]::IsNullOrWhiteSpace($Text)) {
        return "none"
    }

    $normalized = $Text -replace "`r`n", "`n"
    $commits = $normalized -split "(?m)^---PORT_MCP_COMMIT---$"
    $highest = "none"

    foreach ($commit in $commits) {
        $lines = @(($commit -split "`n") | Where-Object { $_ -ne "" })
        if ($lines.Count -eq 0) {
            continue
        }

        $subject = $lines[0]
        $body = if ($lines.Count -gt 1) { ($lines[1..($lines.Count - 1)] -join "`n") } else { "" }

        if ($subject -match '^[a-zA-Z]+(?:\([^)]+\))?!:' -or $body -match '(?m)^BREAKING[ -]CHANGE:') {
            return "major"
        }

        if ($subject -match '^feat(?:\([^)]+\))?:') {
            $highest = "minor"
            continue
        }

        if ($highest -eq "none" -and $subject -match '^(fix|perf|refactor)(?:\([^)]+\))?:') {
            $highest = "patch"
        }
    }

    return $highest
}

$version = Parse-Version -Version $CurrentVersion
$bump = Get-BumpLevel -Text $CommitText

switch ($bump) {
    "major" {
        $nextVersion = "{0}.0.0" -f ($version.Major + 1)
    }
    "minor" {
        $nextVersion = "{0}.{1}.0" -f $version.Major, ($version.Minor + 1)
    }
    "patch" {
        $nextVersion = "{0}.{1}.{2}" -f $version.Major, $version.Minor, ($version.Patch + 1)
    }
    default {
        $nextVersion = $CurrentVersion
    }
}

[pscustomobject]@{
    bump = $bump
    next_version = $nextVersion
    tag = "v$nextVersion"
} | ConvertTo-Json -Compress
