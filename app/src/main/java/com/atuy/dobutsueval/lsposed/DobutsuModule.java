package com.atuy.dobutsueval.lsposed;

import android.app.Activity;
import android.content.pm.ApplicationInfo;
import android.util.Log;
import java.io.File;
import io.github.libxposed.api.XposedModule;
import io.github.libxposed.api.XposedModuleInterface.PackageReadyParam;

/**
 * Modern LSPosed API 102: hook the game Activity lifecycle and load the module
 * native library by absolute module path, not the game's class loader path.
 */
public final class DobutsuModule extends XposedModule {
    private static final String TAG = "DobutsuEval/LSPosed";
    private static final String TARGET = "jp.co.gmode.dobutsushogi";
    private static final String LIB_FILE = "libdobutsuhook.so";
    private boolean installed;

    public DobutsuModule() { super(); }

    @Override
    public void onPackageReady(PackageReadyParam param) {
        if (!TARGET.equals(param.getPackageName()) || installed) return;
        installed = true;

        try {
            hook(Activity.class.getDeclaredMethod("onResume")).intercept(chain -> {
                Object result = chain.proceed();
                Activity activity = (Activity) chain.getThisObject();
                if (TARGET.equals(activity.getPackageName())) {
                    MiniBoardOverlay.onResumed(activity);
                }
                return result;
            });
            hook(Activity.class.getDeclaredMethod("onPause")).intercept(chain -> {
                Activity activity = (Activity) chain.getThisObject();
                if (TARGET.equals(activity.getPackageName())) {
                    MiniBoardOverlay.onPaused(activity);
                }
                return chain.proceed();
            });
            Log.i(TAG, "API 102 Activity hooks installed");
        } catch (ReflectiveOperationException | RuntimeException error) {
            Log.e(TAG, "Failed to install Activity hooks", error);
        }

        loadNativeModule();
    }

    /**
     * System.loadLibrary() resolves against the caller's ClassLoader paths.
     * An LSPosed entry is injected into another application's process, whose
     * native library search path need not include this module's APK libraries.
     * Module ApplicationInfo is provided by the API 102 framework.
     */
    private void loadNativeModule() {
        try {
            Class.forName(GameOverlayController.class.getName(), true,
                    GameOverlayController.class.getClassLoader());
            Class.forName(MiniBoardOverlay.class.getName(), true,
                    MiniBoardOverlay.class.getClassLoader());

            ApplicationInfo info = getModuleApplicationInfo();
            if (info == null || info.nativeLibraryDir == null ||
                    info.nativeLibraryDir.isEmpty()) {
                Log.e(TAG, "getModuleApplicationInfo missing nativeLibraryDir");
                GameOverlayController.onNativeStatus(3, -5);
                return;
            }

            File file = new File(info.nativeLibraryDir, LIB_FILE);
            Log.i(TAG, "module nativeLibraryDir=" + info.nativeLibraryDir +
                    ", exists=" + file.isFile() + ", canRead=" + file.canRead());
            if (!file.isFile()) {
                Log.e(TAG, "Module native library absent: " + file.getAbsolutePath() +
                        " (check arm64 ABI and native library extraction)");
                GameOverlayController.onNativeStatus(3, -3);
                return;
            }

            // The absolute path is associated with the module class loader and
            // triggers JNI_OnLoad and the registered LSPosed native_init.
            System.load(file.getAbsolutePath());
            Log.i(TAG, "Native hook library loaded from module absolute path");
        } catch (UnsatisfiedLinkError error) {
            Log.e(TAG, "Module lib dlopen/JNI_OnLoad failed: " + error.getMessage(), error);
            GameOverlayController.onNativeStatus(3, -4);
        } catch (ReflectiveOperationException | RuntimeException error) {
            Log.e(TAG, "Cannot initialize native observer: " + error.getMessage(), error);
            GameOverlayController.onNativeStatus(3, -6);
        } catch (LinkageError error) {
            Log.e(TAG, "Native observer linkage error: " + error.getMessage(), error);
            GameOverlayController.onNativeStatus(3, -7);
        }
    }
}
