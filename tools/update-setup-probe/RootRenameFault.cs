// Fault injector only: observe an actual root rename, then kill the owned guardian.
// It never moves, extracts or publishes installation files.
using System;
using System.Diagnostics;
using System.IO;
using System.Text.RegularExpressions;
using System.Threading;

public sealed class RootRenameFault : IDisposable {
    readonly FileSystemWatcher watcher;
    readonly ManualResetEventSlim ready = new ManualResetEventSlim(false);
    readonly Process guardian;
    readonly string root, casePath, setupPath, engineRecord;
    Process setup;
    int fired;
    public string Error { get; private set; }
    public string SavedRoot { get; private set; }
    public string ObservedUtc { get; private set; }
    public bool SavedRootExisted { get; private set; }
    public bool SavedFixtureExisted { get; private set; }
    public bool NewRootExisted { get; private set; }
    public bool NewFixtureExisted { get; private set; }
    public int SetupPid { get; private set; }
    public int SetupExit => setup.ExitCode;

    public RootRenameFault(string directory, Process owner, string executable, string marker) {
        casePath = Path.GetFullPath(directory);
        root = Path.Combine(casePath, "installation");
        guardian = owner;
        setupPath = Path.GetFullPath(executable);
        engineRecord = marker;
        watcher = new FileSystemWatcher(casePath) {
            IncludeSubdirectories = false, NotifyFilter = NotifyFilters.DirectoryName
        };
        watcher.Renamed += Observe;
        watcher.Error += (_, e) => Fail(e.GetException());
        watcher.EnableRaisingEvents = true;
    }
    void Fail(Exception error) {
        Error = error.ToString();
        try { if (!guardian.HasExited) guardian.Kill(); }
        catch (Exception cleanup) { Error += "\n" + cleanup; }
        ready.Set();
    }
    void Observe(object sender, RenamedEventArgs e) {
        if (!String.Equals(e.OldFullPath, root, StringComparison.OrdinalIgnoreCase)) return;
        if (Interlocked.CompareExchange(ref fired, 1, 0) != 0) return;
        try {
            SavedRoot = Path.GetFullPath(e.FullPath);
            if (!String.Equals(Path.GetDirectoryName(SavedRoot), casePath, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException("Saved root escaped the disposable case");
            var match = Regex.Match(File.ReadAllText(engineRecord), "\"pid\"\\s*:\\s*([0-9]+)");
            if (!match.Success) throw new InvalidOperationException("Engine identity unavailable at rename");
            SetupPid = Int32.Parse(match.Groups[1].Value);
            setup = Process.GetProcessById(SetupPid);
            var heldHandle = setup.SafeHandle; // Retain before injection and identity inspection.
            if (heldHandle.IsInvalid || !String.Equals(setup.MainModule.FileName, setupPath, StringComparison.OrdinalIgnoreCase)
                || setup.StartTime.ToUniversalTime() < guardian.StartTime.ToUniversalTime())
                throw new InvalidOperationException("Unexpected engine identity at rename");
            ObservedUtc = DateTime.UtcNow.ToString("o");
            SavedRootExisted = Directory.Exists(SavedRoot);
            SavedFixtureExisted = File.Exists(Path.Combine(SavedRoot, "current", "fixture.exe"));
            NewRootExisted = Directory.Exists(root);
            NewFixtureExisted = File.Exists(Path.Combine(root, "current", "fixture.exe"));
            if (!SavedRootExisted || !SavedFixtureExisted || NewFixtureExisted)
                throw new InvalidOperationException("Post-rename fault window missed; evidence is inconclusive");
            guardian.Kill(); // Never kill the engine directly: exercise guardian/job loss.
            ready.Set();
        } catch (Exception error) { Fail(error); }
    }
    public bool Wait(int milliseconds) => ready.Wait(milliseconds);
    public bool WaitForSetupExit(int milliseconds) => setup != null && setup.WaitForExit(milliseconds);
    public void Dispose() {
        watcher.Dispose();
        setup?.Dispose();
        ready.Dispose();
    }
}
