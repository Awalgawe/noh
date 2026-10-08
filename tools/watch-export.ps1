param(
    [Parameter(Mandatory=$true)][string]$LogPath,
    [double]$Duration = 0,
    [int]$ExportPid = 0,
    [string]$EventsPath,
    [string]$CancelPath
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
$form = New-Object System.Windows.Forms.Form
$form.Text = 'Video export progress'
$form.ClientSize = New-Object System.Drawing.Size(540, 145)
$form.StartPosition = 'CenterScreen'
$form.FormBorderStyle = 'FixedDialog'
$form.MaximizeBox = $false
$label = New-Object System.Windows.Forms.Label
$label.SetBounds(20, 20, 500, 30)
$label.Font = New-Object System.Drawing.Font('Segoe UI', 12)
$label.Text = 'Reading progress...'
$bar = New-Object System.Windows.Forms.ProgressBar
$bar.SetBounds(20, 58, 500, 25)
$detail = New-Object System.Windows.Forms.Label
$detail.SetBounds(20, 98, 500, 30)
$detail.Text = 'Closing this window does not stop the export.'
$form.Controls.AddRange(@($label, $bar, $detail))
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 500
# Read the shared controller's append-only event stream incrementally.
# Human-readable logs are diagnostic output, never an execution protocol.
if (-not $EventsPath) { $EventsPath = $LogPath + '.events.jsonl' }
$script:eventReader = $null
$script:pendingEvents = ''
$cancelButton = New-Object System.Windows.Forms.Button
$cancelButton.Text = 'Cancel'
$cancelButton.SetBounds(420, 98, 100, 28)
$cancelButton.Enabled = [bool]$CancelPath
$cancelButton.Add_Click({
    [System.IO.File]::WriteAllText($CancelPath, 'cancel')
    $cancelButton.Enabled = $false
    $detail.Text = 'Cancelling and cleaning temporary files...'
})
$detail.Width = 395
$form.Controls.Add($cancelButton)
$timer.Add_Tick({
    try {
        if (-not $script:eventReader -and [System.IO.File]::Exists($EventsPath)) {
            $stream = [System.IO.File]::Open($EventsPath, 'Open', 'Read', 'ReadWrite')
            $script:eventReader = New-Object System.IO.StreamReader($stream)
        }
        if ($script:eventReader) {
            $script:pendingEvents += $script:eventReader.ReadToEnd()
            $count = 0
            while (($taskNewline = $script:pendingEvents.IndexOf("`n")) -ge 0 -and $count -lt 200) {
                $line = $script:pendingEvents.Substring(0, $taskNewline)
                $script:pendingEvents = $script:pendingEvents.Substring($taskNewline + 1)
                $count++
                if (-not $line) { continue }
                $taskEvent = $line | ConvertFrom-Json
                switch ($taskEvent.event) {
                    'progress' {
                        $bar.Value = [int]$taskEvent.data.percent
                        $label.Text = "$($taskEvent.data.percent) % - Export in progress"
                        $detail.Text = $taskEvent.display
                    }
                    'done' {
                        if ($taskEvent.data.PSObject.Properties.Name -contains 'Ok') {
                            $bar.Value = 100
                            $label.Text = '100 % - Export complete'
                        } else { $label.Text = 'Export failed' }
                        $detail.Text = $taskEvent.display
                        $cancelButton.Enabled = $false
                        $timer.Stop()
                        return
                    }
                    'cancelled' {
                        $label.Text = 'Export cancelled'
                        $detail.Text = 'Temporary files cleaned up.'
                        $cancelButton.Enabled = $false
                        $timer.Stop()
                        return
                    }
                }
            }
        }
        if ($ExportPid -gt 0 -and -not (Get-Process -Id $ExportPid -ErrorAction SilentlyContinue)) {
            $label.Text = 'Export stopped'
            $detail.Text = 'See the log for details.'
            $cancelButton.Enabled = $false
            $timer.Stop()
        }
    } catch { $detail.Text = 'Waiting for progress information...' }
})
$form.Add_Shown({ $timer.Start() })
$form.Add_FormClosed({ $timer.Stop(); $timer.Dispose(); if ($script:eventReader) { $script:eventReader.Dispose() } })
[void]$form.ShowDialog()
