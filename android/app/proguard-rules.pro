# Entry point started by app_process / Shizuku; must keep its name.
-keep class app.syncboard.daemon.Main { public static void main(java.lang.String[]); }
-keep class app.syncboard.daemon.ShizukuService { *; }
