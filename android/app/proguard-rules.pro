# Entry point started by app_process / Shizuku; must keep its name.
-keep class app.syncboard.daemon.Main { public static void main(java.lang.String[]); }
-keep class app.syncboard.daemon.ShizukuService { *; }
# Overrides hidden ContentResolver methods (invisible to R8, so it would strip them).
-keep class app.syncboard.daemon.ShellContext$ExternalResolver { *; }
# Compile-only stub from :hidden-stubs; the real class is in the framework.
-dontwarn android.content.IContentProvider
