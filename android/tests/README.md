# Shell provider lifetime regression

Use a dedicated Android emulator. This test runs the APK's real `ShellContext`
under `app_process` as shell, just like the daemon. It queries the SMS and media
providers and measures external Binder handles retained by ActivityManager while
the test process is still alive. Measuring only the application's heap misses
this leak.

```powershell
cd android
.\gradlew.bat :app:assembleDebug :app:testDebugUnitTest
cd ..
.\android\tests\provider-lifetime.ps1 -Serial emulator-5580 -Rounds 500
```

Requires JDK 17 (`JAVA_HOME`), Android SDK platform/build tools, and a booted
emulator. The script takes `-Sdk` and `-Apk` overrides. The APK must be a debug
build: the probe locates `ShellContext` by its unobfuscated name. It copies the
APK to `/data/local/tmp` without installing it or changing daemon settings.

The test checks:

- 500 SMS + 500 MediaStore queries, including cursor closure;
- overlapping stable and unstable provider clients, retained until closed;
- a query rejected by the provider;
- 200 queries from four concurrent workers.

Before the fix, the initial 1,000 queries on an Android 15 emulator retained
2,000 external handles (`externalHandlesBefore=0 externalHandlesAfter=2000`).
The probe exits unsuccessfully on any increase. Handles being cleaned up only
when the process dies is not a pass.

With the fix, the same Android 15 run reports:

```text
queries=1000 externalHandlesBefore=0 externalHandlesAfter=0 delta=0
after overlapping clients, rejected query, 200 concurrent queries: handles=0
PASS: provider handles returned to baseline
```

The signed, optimized release daemon was also installed and started in the
emulator: default SMS polling over 30 seconds and 100 `imageFolders` IPC requests
left no external provider handles. The old daemon's default polling alone added
20 handles over 30 seconds.

This catches the resource leak, not a vendor-specific system reboot. Confirming
the cause of an iQOO/OriginOS reboot still requires device logs and an extended
run on that device.
