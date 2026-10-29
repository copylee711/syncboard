import android.content.ContentResolver;
import android.content.ContentProviderClient;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.os.Looper;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** Run as shell with the APK on CLASSPATH; measures system-side handles, not app heap. */
public final class ProviderLifetimeProbe {
    private static int handles() throws Exception {
        Process p = new ProcessBuilder("dumpsys", "activity", "providers").start();
        String dump = new String(p.getInputStream().readAllBytes(), java.nio.charset.StandardCharsets.UTF_8);
        if (p.waitFor() != 0) throw new AssertionError("dumpsys failed");
        Matcher m = Pattern.compile("externals: w/token=(\\d+)").matcher(dump);
        int count = 0;
        while (m.find()) count += Integer.parseInt(m.group(1));
        return count;
    }

    public static void main(String[] args) throws Exception {
        Looper.prepareMainLooper();
        Class<?> shell = Class.forName("app.syncboard.daemon.ShellContext");
        Object companion = shell.getField("Companion").get(null);
        Context ctx = (Context) companion.getClass().getMethod("init").invoke(companion);
        ContentResolver resolver = ctx.getContentResolver();
        int rounds = args.length > 0 ? Integer.parseInt(args[0]) : 500;
        int before = handles();
        for (int i = 0; i < rounds; i++) {
            // Same acquire/query/close/release path as SmsWatcher and MediaWatcher.
            try (Cursor c = resolver.query(Uri.parse("content://sms/inbox"),
                    new String[]{"_id"}, null, null, "_id DESC LIMIT 1")) {
                if (c == null) throw new AssertionError("SMS query returned null");
                c.moveToFirst();
            }
            try (Cursor c = resolver.query(Uri.parse("content://media/external/images/media"),
                    new String[]{"_id"}, "_id>?", new String[]{"0"}, "_id ASC")) {
                if (c == null) throw new AssertionError("MediaStore query returned null");
                c.moveToFirst();
            }
        }
        // Do not rely on process death or GC to hide leaked system references.
        int after = handles();
        System.out.println("queries=" + (rounds * 2) + " externalHandlesBefore=" + before
                + " externalHandlesAfter=" + after + " delta=" + (after - before));
        if (after > before) throw new AssertionError("External provider handles leaked while daemon stays alive");
        Uri sms = Uri.parse("content://sms/inbox");
        // Keep two acquisitions alive at once, then close in the opposite order.
        try (ContentProviderClient stable = resolver.acquireContentProviderClient(sms);
             ContentProviderClient unstable = resolver.acquireUnstableContentProviderClient(sms)) {
            if (stable == null || unstable == null) throw new AssertionError("Missing provider client");
            if (handles() <= before) throw new AssertionError("Live clients lost their system references");
            try (Cursor c = stable.query(sms, new String[]{"_id"}, null, null, null)) {
                if (c == null) throw new AssertionError("Stable client stopped working");
                c.moveToFirst();
            }
        }
        // ContentResolver must also release after the provider rejects a query.
        boolean rejected = false;
        try (Cursor ignored = resolver.query(sms, new String[]{"missing_syncboard_test_column"}, null, null, null)) {
        } catch (RuntimeException expected) {
            rejected = true;
        }
        if (!rejected) throw new AssertionError("Failure-path query unexpectedly succeeded");
        java.util.concurrent.ExecutorService pool = java.util.concurrent.Executors.newFixedThreadPool(4);
        try {
            java.util.List<java.util.concurrent.Future<?>> tasks = new java.util.ArrayList<>();
            for (int t = 0; t < 4; t++) tasks.add(pool.submit(() -> {
                for (int i = 0; i < 50; i++) {
                    try (Cursor c = resolver.query(sms, new String[]{"_id"}, null, null, "_id DESC LIMIT 1")) {
                        if (c == null) throw new AssertionError("Concurrent query returned null");
                        c.moveToFirst();
                    }
                }
            }));
            for (java.util.concurrent.Future<?> task : tasks) task.get(60, java.util.concurrent.TimeUnit.SECONDS);
        } finally {
            pool.shutdownNow();
        }
        int finalHandles = handles();
        System.out.println("after overlapping clients, rejected query, 200 concurrent queries: handles=" + finalHandles);
        if (finalHandles > before) throw new AssertionError("Provider lifetime edge cases leaked handles");
        System.out.println("PASS: provider handles returned to baseline");
        System.exit(0);
    }
}
