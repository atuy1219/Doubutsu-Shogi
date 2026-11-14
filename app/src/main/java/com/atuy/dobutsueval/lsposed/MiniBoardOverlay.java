package com.atuy.dobutsueval.lsposed;

import android.app.Activity;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.lang.ref.WeakReference;
import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.zip.GZIPInputStream;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/**
 * Single LSPosed in-game overlay. Graph is open by default; 3x4 mini-board
 * is opt-in. Both panels can independently collapse while keeping the
 * controls visible. No SYSTEM_ALERT_WINDOW or screenshot permission.
 *
 * The graph records one point per CHANGED, validated board+hands snapshot,
 * never per AI search invocation or raw renderer invocation.
 */
public final class MiniBoardOverlay {
    private static final String TAG = "DobutsuEval/Board";
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final Object LOCK = new Object();
    private static final int UNKNOWN = Integer.MIN_VALUE;
    private static final int MAX_HISTORY = 220;
    private static final long TABLEBASE_BYTES = 167_527_978L;
    private static final String TABLEBASE_SHA256 =
            "95936ff86f6cc4958e39e8d98bc82fef304315985ce34a8bb26602f4d89acabb";
    private static final AtomicBoolean TABLEBASE_INSTALLING = new AtomicBoolean(false);
    private static volatile String moduleApkPath;
    private static final int GREEN = Color.rgb(84, 222, 164);
    private static final int RED = Color.rgb(251, 136, 121);
    private static final int PAPER = Color.rgb(242, 226, 186);
    private static final int PANEL = Color.rgb(21, 26, 37);
    private static final String[] PIECES = {"ラ", "キ", "ゾ", "ヒ", "ニ"};

    private static WeakReference<Activity> active = new WeakReference<>(null);
    private static LinearLayout panel;
    private static TextView graphToggle;
    private static TextView boardToggle;
    private static GraphView graphView;
    private static BoardView boardView;

    private static boolean graphExpanded = true;
    private static boolean boardExpanded = false;
    private static int leftOffset = -1, topOffset = -1;

    private static int[] board, hands;
    private static int rustScore = UNKNOWN;
    private static boolean exactScore = false;
    private static int verifiedCompletedMove = -1;
    private static int verifiedSideToMove = -1;
    private static long nextSampleId = 0;
    private static long epoch = 0;
    private static final ArrayList<Sample> history = new ArrayList<>();
    private static int nativeStatus = 0, nativeDetails = 0;

    private static final class Sample {
        final long id;
        final int score;
        Sample(long id, int score) { this.id = id; this.score = score; }
    }

    private MiniBoardOverlay() {}

    private static native void nativeSetTablebasePath(String path);

    public static void setModuleApkPath(String path) {
        moduleApkPath = path;
    }

    private static boolean isVerifiedTablebase(File table) {
        if (!table.isFile() || table.length() != TABLEBASE_BYTES) return false;
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            try (InputStream input = new BufferedInputStream(new FileInputStream(table))) {
                byte[] buffer = new byte[1024 * 1024];
                int read;
                while ((read = input.read(buffer)) != -1) digest.update(buffer, 0, read);
            }
            return TABLEBASE_SHA256.equals(hex(digest.digest()));
        } catch (IOException | NoSuchAlgorithmException error) {
            return false;
        }
    }

    private static String hex(byte[] bytes) {
        StringBuilder out = new StringBuilder(bytes.length * 2);
        for (byte value : bytes) out.append(String.format("%02x", value & 0xff));
        return out.toString();
    }

    private static void setNativeTablebasePath(File table) {
        try {
            nativeSetTablebasePath(table.getAbsolutePath());
        } catch (UnsatisfiedLinkError error) {
            Log.e(TAG, "Native tablebase path setter is unavailable", error);
        }
    }

    private static void prepareBundledTablebase(Activity activity) {
        File table = new File(activity.getFilesDir(), "dobutsu.ref");
        if (!TABLEBASE_INSTALLING.compareAndSet(false, true)) return;

        final String apkPath = moduleApkPath;
        Thread installer = new Thread(() -> {
            File temporary = new File(table.getParentFile(), "dobutsu.ref.installing");
            try {
                if (isVerifiedTablebase(table)) {
                    setNativeTablebasePath(table);
                    Log.i(TAG, "Existing bundled WDL/DTM tablebase verified");
                    return;
                }
                if (apkPath == null || apkPath.isEmpty()) {
                    throw new IOException("Module APK path is unavailable");
                }
                if (!table.getParentFile().isDirectory() &&
                        !table.getParentFile().mkdirs()) {
                    throw new IOException("Cannot create game files directory");
                }
                if (temporary.exists() && !temporary.delete()) {
                    throw new IOException("Cannot remove stale tablebase temporary file");
                }

                MessageDigest digest = MessageDigest.getInstance("SHA-256");
                long total = 0;
                try (ZipFile moduleApk = new ZipFile(apkPath)) {
                    ZipEntry asset = moduleApk.getEntry("assets/dobutsu.tablebase");
                    if (asset == null) throw new IOException("Bundled tablebase asset is missing");
                    try (InputStream compressed = new BufferedInputStream(
                                moduleApk.getInputStream(asset));
                         GZIPInputStream input = new GZIPInputStream(compressed);
                         FileOutputStream fileOutput = new FileOutputStream(temporary);
                         BufferedOutputStream output = new BufferedOutputStream(
                                fileOutput, 1024 * 1024)) {
                        byte[] buffer = new byte[1024 * 1024];
                        int read;
                        while ((read = input.read(buffer)) != -1) {
                            total += read;
                            if (total > TABLEBASE_BYTES) {
                                throw new IOException("Tablebase exceeds expected size");
                            }
                            digest.update(buffer, 0, read);
                            output.write(buffer, 0, read);
                        }
                        output.flush();
                        fileOutput.getFD().sync();
                    }
                }

                String actualHash = hex(digest.digest());
                if (total != TABLEBASE_BYTES || !TABLEBASE_SHA256.equals(actualHash)) {
                    throw new IOException("Tablebase integrity check failed: bytes=" +
                            total + " sha256=" + actualHash);
                }

                if (table.exists() && !table.delete()) {
                    throw new IOException("Cannot replace previous tablebase");
                }
                if (!temporary.renameTo(table)) {
                    throw new IOException("Cannot atomically install verified tablebase");
                }

                setNativeTablebasePath(table);
                Log.i(TAG, "Bundled exact WDL/DTM tablebase installed: " +
                        table.length() + " bytes");
            } catch (IOException | NoSuchAlgorithmException | RuntimeException error) {
                if (temporary.exists()) temporary.delete();
                Log.e(TAG, "Bundled exact tablebase installation failed", error);
            } finally {
                TABLEBASE_INSTALLING.set(false);
            }
        }, "Dobutsu-Tablebase-Install");
        installer.setPriority(Thread.NORM_PRIORITY - 1);
        installer.start();
    }

    private static int dp(Activity a, float value) {
        return Math.round(value * a.getResources().getDisplayMetrics().density);
    }

    private static GradientDrawable background(int color, float radius, Activity a) {
        GradientDrawable b = new GradientDrawable();
        b.setColor(color);
        b.setCornerRadius(dp(a, radius));
        return b;
    }

    public static void onResumed(Activity activity) {
        prepareBundledTablebase(activity);
        MAIN.post(() -> {
            if (activity.isFinishing() || activity.isDestroyed()) return;
            if (active.get() != activity) detach();
            active = new WeakReference<>(activity);
            if (panel != null) return;
            try {
                FrameLayout decor = (FrameLayout) activity.getWindow().getDecorView();
                LinearLayout root = new LinearLayout(activity);
                root.setOrientation(LinearLayout.VERTICAL);
                root.setPadding(dp(activity, 7), dp(activity, 4),
                        dp(activity, 7), dp(activity, 7));
                root.setBackground(background(Color.argb(247, 21, 26, 37), 14, activity));

                LinearLayout controls = new LinearLayout(activity);
                controls.setGravity(Gravity.CENTER_VERTICAL);
                graphToggle = toggle(activity);
                boardToggle = toggle(activity);
                TextView drag = toggle(activity);
                drag.setText("⋮⋮");
                drag.setTextColor(Color.rgb(160, 175, 195));
                drag.setGravity(Gravity.CENTER);
                controls.addView(graphToggle, new LinearLayout.LayoutParams(0, dp(activity, 39), 1f));
                controls.addView(boardToggle, new LinearLayout.LayoutParams(0, dp(activity, 39), 1f));
                controls.addView(drag, new LinearLayout.LayoutParams(dp(activity, 40), dp(activity, 39)));
                root.addView(controls);

                graphView = new GraphView(activity);
                root.addView(graphView, new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, dp(activity, 174)));
                boardView = new BoardView(activity);
                root.addView(boardView, new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, dp(activity, 235)));

                panel = root;
                graphToggle.setOnClickListener(v -> {
                    graphExpanded = !graphExpanded;
                    updateVisibility();
                });
                boardToggle.setOnClickListener(v -> {
                    boardExpanded = !boardExpanded;
                    updateVisibility();
                });
                installDragHandle(drag, decor, activity);
                FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
                        dp(activity, 315), ViewGroup.LayoutParams.WRAP_CONTENT,
                        Gravity.TOP | Gravity.LEFT);
                int screen = activity.getResources().getDisplayMetrics().widthPixels;
                params.leftMargin = leftOffset >= 0 ? leftOffset :
                        Math.max(0, screen - dp(activity, 325));
                params.topMargin = topOffset >= 0 ? topOffset : dp(activity, 65);
                decor.addView(panel, params);
                updateVisibility();
                Log.i(TAG, "Rust evaluation graph and toggleable 3x4 board attached");
            } catch (Exception e) {
                Log.e(TAG, "Cannot attach in-game overlay", e);
                detach();
            }
        });
    }

    private static TextView toggle(Activity a) {
        TextView button = new TextView(a);
        button.setTextSize(13f);
        button.setTypeface(Typeface.DEFAULT_BOLD);
        button.setGravity(Gravity.CENTER);
        button.setClickable(true);
        return button;
    }

    private static void updateVisibility() {
        if (panel == null) return;
        graphToggle.setText(graphExpanded ? "形勢 ▾" : "形勢 ▸");
        boardToggle.setText(boardExpanded ? "盤面 ▾" : "盤面 ▸");
        graphToggle.setTextColor(graphExpanded ? GREEN : Color.rgb(179, 194, 210));
        boardToggle.setTextColor(boardExpanded ? GREEN : Color.rgb(179, 194, 210));
        graphView.setVisibility(graphExpanded ? View.VISIBLE : View.GONE);
        boardView.setVisibility(boardExpanded ? View.VISIBLE : View.GONE);
        if (graphExpanded) graphView.invalidate();
        if (boardExpanded) boardView.invalidate();
    }

    private static void installDragHandle(View drag, FrameLayout parent, Activity a) {
        final float[] down = new float[2];
        final int[] start = new int[2];
        drag.setOnTouchListener((v, event) -> {
            if (panel == null) return false;
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    down[0] = event.getRawX();
                    down[1] = event.getRawY();
                    FrameLayout.LayoutParams atDown =
                            (FrameLayout.LayoutParams) panel.getLayoutParams();
                    start[0] = atDown.leftMargin;
                    start[1] = atDown.topMargin;
                    return true;
                case MotionEvent.ACTION_MOVE: {
                    FrameLayout.LayoutParams p =
                            (FrameLayout.LayoutParams) panel.getLayoutParams();
                    p.leftMargin = Math.max(0, Math.min(
                            Math.max(0, parent.getWidth() - panel.getWidth()),
                            start[0] + Math.round(event.getRawX() - down[0])));
                    p.topMargin = Math.max(0, Math.min(
                            Math.max(0, parent.getHeight() - panel.getHeight()),
                            start[1] + Math.round(event.getRawY() - down[1])));
                    panel.setLayoutParams(p);
                    return true;
                }
                case MotionEvent.ACTION_UP:
                    FrameLayout.LayoutParams p =
                            (FrameLayout.LayoutParams) panel.getLayoutParams();
                    leftOffset = p.leftMargin;
                    topOffset = p.topMargin;
                    return true;
                case MotionEvent.ACTION_CANCEL:
                    return true;
                default:
                    return false;
            }
        });
    }

    public static void onPaused(Activity activity) {
        MAIN.post(() -> { if (active.get() == activity) detach(); });
    }

    private static void detach() {
        if (panel != null) {
            ViewGroup parent = (ViewGroup) panel.getParent();
            if (parent != null) parent.removeView(panel);
        }
        panel = null;
        graphView = null;
        boardView = null;
        graphToggle = null;
        boardToggle = null;
        active.clear();
    }

    // bit0 changes on certain game UI/rotation paths and is not the piece
    // identity. Compare canonical board states, not raw renderer flags.
    private static boolean sameCanonicalBoard(int[] a, int[] b) {
        if (a == null || b == null || a.length != b.length) return false;
        for (int i=0;i<a.length;i++) {
            int x = a[i]==-1 ? -1 : a[i]&~1;
            int y = b[i]==-1 ? -1 : b[i]&~1;
            if (x!=y) return false;
        }
        return true;
    }

    private static boolean sameHandContents(int[] a, int[] b) {
        if (a==null || b==null) return a==b;
        // Hands are multisets. Slot ordering and render/rotation flags
        // must not produce a spurious new "move" on the graph.
        int[] counts=new int[64];
        for(int raw:a) if(raw!=-1) {
            int code=raw&~1;
            if(code<0 || code>=counts.length)return false;
            counts[code]++;
        }
        for(int raw:b) if(raw!=-1) {
            int code=raw&~1;
            if(code<0 || code>=counts.length)return false;
            counts[code]--;
        }
        for(int c:counts) if(c!=0)return false;
        return true;
    }

    /**
     * Native supplies the completed ply from the exact drawTekazu counter.
     * The side-to-move is supplied only after its game-model encoding has
     * been calibrated against an actual observed committed move.
     *
     * Never substitute callback counts for an unverified game move number.
     */
    public static void onNativeBoard(int[] rawBoard, int[] rawHands,
                                     int[] ignoredConstants, int score,
                                     int completedMove, int sideToMove, int isExact) {
        if (isExact != 1 || rawBoard == null || rawBoard.length != 12 ||
                score == UNKNOWN) return;
        synchronized (LOCK) {
            boolean boardChanged = !sameCanonicalBoard(board,rawBoard) ||
                                   !sameHandContents(hands,rawHands);
            boolean scoreChanged = rustScore != score;
            boolean useExact = isExact == 1;
            boolean modeChanged = exactScore != useExact;
            boolean metaChanged = verifiedCompletedMove != completedMove ||
                                  verifiedSideToMove != sideToMove;
            if (!boardChanged && !scoreChanged && !metaChanged &&
                !modeChanged) return;
            if (modeChanged && board != null) {
                // Never draw a line mixing exact WDL/DTM and heuristic points.
                history.clear();
                nextSampleId = 0;
                epoch++;
            }
            board = rawBoard.clone();
            hands = rawHands == null ? null : rawHands.clone();
            rustScore = score;
            exactScore = useExact;
            verifiedCompletedMove = completedMove;
            verifiedSideToMove = sideToMove;

            // One actual completed game move = at most one graph point.
            // Missing UI counters are *not* backfilled with fake points.
            if(completedMove >= 1 && completedMove <= 10000) {
                int count = history.size();
                if(count==0 || completedMove > history.get(count-1).id) {
                    history.add(new Sample(completedMove,score));
                    nextSampleId = completedMove;
                    if(history.size()>MAX_HISTORY)history.remove(0);
                } else if(completedMove == history.get(count-1).id) {
                    // The terminal screen can show the final evaluated
                    // position BEFORE the UI increments its "Nてめ" label.
                    // The final board and the last graph point legitimately
                    // share the same completed move number; always replace
                    // its score, even if the board has changed in this frame.
                    // This is a REPLACEMENT, never a second point.
                    history.set(count-1,new Sample(completedMove,score));
                }
                // Stale UI redraw of an older move is intentionally ignored.
            }
        }
        refresh();
    }

    public static void onNewGame() {
        synchronized (LOCK) {
            board = null;
            hands = null;
            rustScore = UNKNOWN;
            exactScore = false;
            verifiedCompletedMove = -1;
            verifiedSideToMove = -1;
            history.clear();
            nextSampleId = 0;
            epoch++;
        }
        refresh();
    }

    public static void onNativeBoardStatus(int state, int details) {
        synchronized (LOCK) {
            nativeStatus = state;
            nativeDetails = details;
        }
        if (state == 3) Log.w(TAG, "Board reader unavailable: " + details);
        refresh();
    }

    private static void refresh() {
        MAIN.post(() -> {
            if (graphView != null) graphView.invalidate();
            if (boardView != null && boardExpanded) boardView.invalidate();
        });
    }

    private static int kind(int raw) {
        if (raw < 0 || raw > 0x3f) return -1;
        switch (raw & 0x1e) {
            case 0x00: return 0; // lion; 0 is NEVER empty
            case 0x02: return 1;
            case 0x04: return 2;
            case 0x06: return 3;
            case 0x08: return 4;
            default: return -1;
        }
    }

    private static String signed(int n) { return n > 0 ? "+" + n : Integer.toString(n); }
    private static int scoreColor(int score) { return score >= 0 ? GREEN : RED; }

    private static final class GraphView extends View {
        private final Paint p = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF r = new RectF();
        private final float d;
        private long selectedId = -1, selectedEpoch = -1;
        private final List<Sample> snapshot = new ArrayList<>();
        private long snapEpoch;
        private float[] dotsX = new float[0], dotsY = new float[0];
        private float downX, downY;

        GraphView(Activity a) {
            super(a);
            d = getResources().getDisplayMetrics().density;
            setClickable(true);
        }

        private float left() { return 14 * d; }
        private float right() { return getWidth() - 13 * d; }
        private float top() { return 39 * d; }
        private float bottom() { return getHeight() - 11 * d; }
        private float y(int score) {
            float f = Math.max(-1, Math.min(1, score / 2000f));
            return (top() + bottom()) / 2f - f * (bottom() - top()) / 2f;
        }
        private float x(int i, int count) {
            if(count<2)return right();
            long move=snapshot.get(i).id;
            long last=snapshot.get(count-1).id;
            if(last<=1)return right();
            return left()+(move-1f)*(right()-left())/(last-1f);
        }
        @Override protected void onDraw(Canvas c) {
            super.onDraw(c);
            int latest;
            boolean ready;
            boolean isExact;
            synchronized (LOCK) {
                snapshot.clear();
                snapshot.addAll(history);
                snapEpoch = epoch;
                latest = rustScore;
                isExact = exactScore;
                ready = board != null && rustScore != UNKNOWN;
            }
            float l = left(), rr = right(), t = top(), b = bottom();
            float mid = (t + b) / 2f;
            p.setStyle(Paint.Style.FILL);
            p.setTypeface(Typeface.DEFAULT);
            p.setTextSize(10 * d);
            p.setColor(Color.rgb(167, 184, 203));
            c.drawText("完全解析 WDL/DTM", l + 4 * d, 17 * d, p);

            String val = ready ? signed(latest) : "--";
            p.setTypeface(Typeface.DEFAULT_BOLD);
            p.setTextSize(15 * d);
            p.setColor(ready ? scoreColor(latest) : Color.WHITE);
            c.drawText(val, rr - p.measureText(val), 33 * d, p);

            p.setColor(Color.argb(24, 43, 209, 149));
            c.drawRect(l, t, rr, mid, p);
            p.setColor(Color.argb(24, 241, 117, 103));
            c.drawRect(l, mid, rr, b, p);
            p.setColor(Color.rgb(121, 136, 157));
            p.setStyle(Paint.Style.STROKE);
            p.setStrokeWidth(d);
            c.drawRect(l, t, rr, b, p);
            c.drawLine(l, mid, rr, mid, p);
            p.setStyle(Paint.Style.FILL);
            p.setColor(Color.rgb(154, 171, 191));
            p.setTextSize(9 * d);
            c.drawText("+2000", l + 4 * d, t + 11 * d, p);
            c.drawText("0", l + 4 * d, mid - 4 * d, p);
            c.drawText("-2000", l + 4 * d, b - 5 * d, p);

            if (snapshot.isEmpty()) {
                p.setTextSize(11 * d);
                p.setColor(Color.rgb(196, 211, 227));
                String message;
                if(!ready)message="完全解析結果待ち";
                else if(verifiedCompletedMove<0)message="手数確認中";
                else message="次の完全解析結果待ち";
                c.drawText(message,l+9*d,mid+19*d,p);
                dotsX = dotsY = new float[0];
                return;
            }
            int count = snapshot.size();
            if (dotsX.length != count) {
                dotsX = new float[count];
                dotsY = new float[count];
            }
            p.setStrokeWidth(2.2f * d);
            p.setStrokeCap(Paint.Cap.ROUND);
            for (int i = 0; i < count; i++) {
                Sample sample = snapshot.get(i);
                dotsX[i] = x(i, count);
                dotsY[i] = y(sample.score);
                if (i > 0 && snapshot.get(i-1).id + 1 == sample.id) {
                    p.setStyle(Paint.Style.STROKE);
                    p.setColor(scoreColor(sample.score));
                    c.drawLine(dotsX[i-1], dotsY[i-1], dotsX[i], dotsY[i], p);
                }
            }
            for (int i = 0; i < count; i++) {
                p.setStyle(Paint.Style.FILL);
                p.setColor(PANEL);
                c.drawCircle(dotsX[i], dotsY[i], 4.3f * d, p);
                p.setStyle(Paint.Style.STROKE);
                p.setStrokeWidth(2.2f * d);
                p.setColor(scoreColor(snapshot.get(i).score));
                c.drawCircle(dotsX[i], dotsY[i], 4.3f * d, p);
            }
            if (selectedEpoch == snapEpoch) {
                for (int i=0; i<count; i++) if (snapshot.get(i).id == selectedId) {
                    paintBubble(c, i);
                    break;
                }
            }
        }

        private void paintBubble(Canvas c, int i) {
            Sample sample = snapshot.get(i);
            float bx = dotsX[i], by = dotsY[i];
            p.setStyle(Paint.Style.STROKE);
            p.setStrokeWidth(1.8f * d);
            p.setColor(Color.WHITE);
            c.drawCircle(bx, by, 7.2f * d, p);
            p.setStyle(Paint.Style.FILL);
            String label = "#" + sample.id + "  " + signed(sample.score);
            p.setTypeface(Typeface.DEFAULT_BOLD);
            p.setTextSize(11 * d);
            float w = p.measureText(label) + 20 * d, h = 27 * d;
            float left = Math.max(3*d, Math.min(getWidth()-w-3*d, bx-w/2));
            float y = by - h - 11*d;
            if (y < 2*d) y = Math.min(getHeight()-h-2*d, by+11*d);
            r.set(left,y,left+w,y+h);
            p.setColor(Color.rgb(39,46,61));
            c.drawRoundRect(r,6*d,6*d,p);
            p.setStyle(Paint.Style.STROKE);
            p.setColor(scoreColor(sample.score));
            p.setStrokeWidth(d);
            c.drawRoundRect(r,6*d,6*d,p);
            p.setStyle(Paint.Style.FILL);
            p.setColor(Color.WHITE);
            c.drawText(label,left+10*d,y+18*d,p);
        }

        @Override public boolean onTouchEvent(MotionEvent e) {
            switch(e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    downX = e.getX();
                    downY = e.getY();
                    return true;
                case MotionEvent.ACTION_UP: {
                    if (Math.hypot(e.getX()-downX,e.getY()-downY) > 12*d) return true;
                    int best = -1;
                    float bestDistance = 18*d*18*d;
                    for(int i=0;i<dotsX.length;i++) {
                        float dx=e.getX()-dotsX[i], dy=e.getY()-dotsY[i];
                        float dist=dx*dx+dy*dy;
                        if (dist<bestDistance) { best=i;bestDistance=dist; }
                    }
                    selectedEpoch = snapEpoch;
                    selectedId = best < 0 ? -1 : snapshot.get(best).id;
                    invalidate();
                    return true;
                }
                case MotionEvent.ACTION_CANCEL: return true;
                default: return true;
            }
        }
    }

    private static final class BoardView extends View {
        private final Paint p = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF r = new RectF();
        private final float d;
        BoardView(Activity a) {
            super(a);
            d = getResources().getDisplayMetrics().density;
        }

        @Override protected void onDraw(Canvas c) {
            super.onDraw(c);
            int[] cells, held;
            int score;
            synchronized(LOCK) {
                cells = board == null ? null : board.clone();
                held = hands == null ? null : hands.clone();
                score = rustScore;
            }
            float square = Math.min((getWidth()-25*d)/3f, 45*d);
            float start = (getWidth()-3*square)/2f;
            float top=8*d;
            p.setTypeface(Typeface.DEFAULT_BOLD);
            p.setTextAlign(Paint.Align.CENTER);
            for(int row=0;row<4;row++) for(int col=0;col<3;col++) {
                int index=row*3+col;
                float x=start+col*square, y=top+row*square;
                p.setStyle(Paint.Style.FILL);
                p.setColor((row+col)%2==0?PAPER:Color.rgb(225,204,163));
                r.set(x+1*d,y+1*d,x+square-1*d,y+square-1*d);
                c.drawRoundRect(r,3*d,3*d,p);
                if(cells==null)continue;
                int raw=cells[index];
                if(raw==-1)continue;
                int kind=kind(raw);
                String text=kind<0?"0x"+Integer.toHexString(raw):PIECES[kind];
                p.setColor(kind<0?Color.rgb(104,92,78):
                    (raw&0x20)==0?Color.rgb(29,128,93):Color.rgb(177,69,75));
                p.setTextSize((kind<0?10:23)*d);
                c.drawText(text,x+square/2f,y+square/2f+(kind<0?3:8)*d,p);
            }
            p.setTextAlign(Paint.Align.LEFT);
            p.setTypeface(Typeface.DEFAULT);
            p.setTextSize(10*d);
            p.setColor(Color.rgb(196,211,227));
            if(cells==null) {
                c.drawText("対局開始待ち",12*d,top+4*square+20*d,p);
            } else {
                StringBuilder h = new StringBuilder();
                if(held!=null) for(int raw:held) if(raw!=-1 && kind(raw)>=0) {
                    if(h.length()>0)h.append(' ');
                    h.append((raw&0x20)==0?'▲':'▽').append(PIECES[kind(raw)]);
                }
                c.drawText(h.length()==0?"持ち駒なし":h.toString(),
                    12*d,top+4*square+20*d,p);
            }
        }
    }
}
