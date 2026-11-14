package com.atuy.dobutsueval.lsposed;

import android.app.Activity;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import java.lang.ref.WeakReference;
import java.util.ArrayList;
import java.util.List;

/**
 * Runs entirely inside the scoped game's process. A child of the game's decor
 * view cannot appear on the launcher or in other apps and needs no overlay
 * permission, background service, socket, notification or accessibility service.
 */
public final class GameOverlayController {
    private static final String TAG = "DobutsuEval/Overlay";
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final Object LOCK = new Object();
    private static final int MAX_POINTS = 180;
    private static final List<Integer> SCORES = new ArrayList<>();
    // For committed ShogiControl moves, keep paired diagnostics without
    // implying that Get_Game_Score and Get_AI_Score have identical units.
    private static final List<Integer> GAME_SCORES = new ArrayList<>();
    private static final List<Integer> MOVE_INDEXES = new ArrayList<>();
    private static WeakReference<Activity> active = new WeakReference<>(null);
    private static Graph graph;
    private static int source = -1;
    private static int state = 0;
    private static int stateDetails = 0;
    private static final int[] EVENT_COUNTS = new int[4];
    // Fixed visualization convention requested by user: raw negative = player-positive.
    // This orientation is not yet verified against alternating-turn positions.
    private static final int SCORE_ORIENTATION = -1;
    // IDs ensure a tapped point remains selected as new samples arrive.
    private static long sampleEpoch = 0;
    private static long firstSampleId = 0;
    private static long nextSampleId = 0;
    private static int offsetX = -1;
    private static int offsetY = -1;

    private GameOverlayController() { }

    public static void onGameResumed(Activity activity) {
        MAIN.post(() -> {
            if (activity.isFinishing() || activity.isDestroyed()) return;
            Activity old = active.get();
            if (old != activity) detach();
            active = new WeakReference<>(activity);
            if (graph != null) return;
            try {
                FrameLayout decor = (FrameLayout) activity.getWindow().getDecorView();
                Graph view = new Graph(activity);
                int width = dp(activity, 300);
                int height = dp(activity, 150);
                FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(width, height,
                        Gravity.TOP | Gravity.LEFT);
                int available = activity.getResources().getDisplayMetrics().widthPixels;
                lp.leftMargin = Math.max(0, offsetX < 0 ? available - width - dp(activity, 12) : offsetX);
                lp.topMargin = Math.max(0, offsetY < 0 ? dp(activity, 70) : offsetY);
                decor.addView(view, lp);
                graph = view;
                view.invalidate();
                Log.i(TAG, "In-game overlay attached: " + activity.getClass().getName());
            } catch (Exception e) {
                Log.e(TAG, "Failed to attach in-game graph", e);
            }
        });
    }

    public static void onGamePaused(Activity activity) {
        MAIN.post(() -> {
            if (active.get() == activity) detach();
        });
    }

    private static void detach() {
        if (graph != null) {
            ViewGroup parent = (ViewGroup) graph.getParent();
            if (parent != null) parent.removeView(graph);
            graph = null;
        }
        active.clear();
    }

    // Called by JNI from the game's IL2CPP thread; never mutate Views here.
    // Raw game/AI scores are not perfect-play evaluations and are never shown.
    public static void onNativeScore(int value, int which) { }

    public static void onNativeCommittedScore(int aiScore, int uiTurn, int gameScore) { }

    /**
     * Captures one point per observed ShogiControl.Cmd_MakeMove command.
     * The score is from Get_AI_Score, and the second value is Get_Game_Score
     * (diagnostic). Neither score has a verified player-side perspective yet.
     */
    /** Called by JNI on the game's new-battle/reset path, not Activity.onResume. */
    public static void onNativeNewMatch(int trigger) {
        synchronized (LOCK) {
            SCORES.clear();
            GAME_SCORES.clear();
            MOVE_INDEXES.clear();
            source = -1;
            sampleEpoch++;
            firstSampleId = 0;
            nextSampleId = 0;
            java.util.Arrays.fill(EVENT_COUNTS, 0);
        }
        Log.i(TAG, "Evaluation history cleared for new match (trigger=" + trigger + ")");
        MAIN.post(() -> {
            if (graph != null) graph.invalidate();
        });
    }

    // 1 = JNI ready, 2 = native hooks installed, 3 = could not attach hooks.
    public static void onNativeStatus(int nextState, int details) {
        synchronized (LOCK) {
            state = nextState;
            stateDetails = details;
        }
        MAIN.post(() -> { if (graph != null) graph.invalidate(); });
    }

    public static void onNativeActivity(int which, int count) {
        if (which >= 0 && which < EVENT_COUNTS.length) {
            synchronized (LOCK) { EVENT_COUNTS[which] = count; }
            MAIN.post(() -> { if (graph != null) graph.invalidate(); });
        }
    }

    private static int dp(Activity activity, float d) {
        return Math.round(activity.getResources().getDisplayMetrics().density * d);
    }

    private static String statusText(int code) {
        switch (code) {
            case -7: return "Java/JNIリンクエラー";
            case -6: return "初期化例外（logcat参照）";
            case -5: return "モジュールのlibパス不明";
            case -4: return "dlopen / JNI_OnLoad失敗";
            case -3: return "モジュールSO未抽出";
            case -2: return "LSPosed Native APIなし";
            case -1: return "ライブラリ読込失敗";
            case 100: return "libil2cpp.so未検出";
            case 101: return "監視スレッド起動失敗";
            case 105: return "libil2cpp.soを検出";
            case 110: return "IL2CPP API不足";
            case 120: return "IL2CPP初期化待ち";
            case 130: return "IL2CPPスレッド登録失敗";
            case 140: return "Assembly-CSharp未発見";
            case 150: return "ShogiModel/Control未発見";
            case 160: return "評価値メソッド未発見";
            case 170: return "関数アドレス取得失敗";
            case 180: return "ネイティブフック拒否";
            case 190: return "Hook設置数0";
            case 210: return "IL2CPP初期化待ち";
            case 211: return "IL2CPP初期化フック失敗";
            case 212: return "初期化元関数不明";
            case 213: return "IL2CPP初期化失敗";
            case 220: return "IL2CPP初期化完了";
            default: return "原因未特定";
        }
    }

    // Legacy diagnostics view; the active in-game graph lives in MiniBoardOverlay.
    private static final class Graph extends View {
        private static final int COLOR_FAVORABLE = Color.rgb(84, 222, 164);
        private static final int COLOR_UNFAVORABLE = Color.rgb(251, 136, 121);
        private final Paint p = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final float density;
        private final List<Integer> snapshot = new ArrayList<>();
        private final List<Integer> snapshotGameScores = new ArrayList<>();
        private final List<Integer> snapshotMoveIndexes = new ArrayList<>();
        private final RectF r = new RectF();

        private float downRawX, downRawY;
        private int startX, startY;
        private boolean dragged;
        private long selectedId = -1;
        private long selectedEpoch = -1;
        private long snapshotEpoch = -1;
        private long snapshotFirstId = 0;
        // Fixed vertical range: +2000 (player) to -2000 (opponent).
        // Raw values outside this range are preserved and drawn at the edge.
        private static final double FIXED_SCALE = 2000d;
        private double scale = FIXED_SCALE;

        Graph(Activity context) {
            super(context);
            density = getResources().getDisplayMetrics().density;
            setClickable(true);
        }

        private float plotLeft() { return 16f*density; }
        private float plotRight() { return getWidth()-16f*density; }
        private float plotTop() { return 47f*density; }
        private float plotBottom() { return getHeight()-14f*density; }
        private float xFor(int index, int count) {
            if(count<=1)return plotRight();
            // Preserve gaps for missed game turns; never shift later samples
            // left to make the observation stream look complete.
            final int current=snapshotMoveIndexes.get(index);
            final int last=snapshotMoveIndexes.get(count-1);
            if(current>0 && last>1)
                return plotLeft()+(current-1f)*(plotRight()-plotLeft())/(last-1f);
            return plotLeft()+index*(plotRight()-plotLeft())/(count-1f);
        }
        private float yFor(int value) {
            final double oriented = -(double)value;
            final double normalized = Math.max(-1d,Math.min(1d,oriented/scale));
            return (float)((plotTop()+plotBottom())/2d -
                    normalized*(plotBottom()-plotTop())/2d);
        }
        private static String signedScore(int raw) {
            long normalized = -(long)raw;
            return (normalized > 0 ? "+" : "") + normalized;
        }
        private static int scoreColor(int raw) {
            return raw <= 0 ? COLOR_FAVORABLE : COLOR_UNFAVORABLE;
        }

        private void copyLatestSnapshot() {
            synchronized (LOCK) {
                snapshot.clear();
                snapshot.addAll(SCORES);
                snapshotGameScores.clear();
                snapshotGameScores.addAll(GAME_SCORES);
                snapshotMoveIndexes.clear();
                snapshotMoveIndexes.addAll(MOVE_INDEXES);
                snapshotEpoch = sampleEpoch;
                snapshotFirstId = firstSampleId;
            }
            // Never autoscale: preserve comparability across moves and games.
            // The display position is saturated; stored score stays unchanged.
            scale = FIXED_SCALE;
        }

        @Override protected void onDraw(Canvas canvas) {
            super.onDraw(canvas);
            final float w=getWidth(), h=getHeight();
            final float left=plotLeft(), right=plotRight();
            final float top=plotTop(), bottom=plotBottom();
            final float mid=(top+bottom)/2f;

            p.setStyle(Paint.Style.FILL);
            p.setColor(Color.rgb(21,26,37));
            r.set(0,0,w,h);
            canvas.drawRoundRect(r,12*density,12*density,p);

            copyLatestSnapshot();
            int currentState, detail;
            int[] activity=new int[4];
            synchronized(LOCK) {
                currentState=state;
                detail=stateDetails;
                System.arraycopy(EVENT_COUNTS,0,activity,0,activity.length);
            }

            final int last = snapshot.isEmpty() ? 0 : snapshot.get(snapshot.size()-1);
            final int lastColor=scoreColor(last);
            p.setTypeface(android.graphics.Typeface.DEFAULT_BOLD);
            p.setTextSize(14f*density);
            p.setColor(Color.WHITE);
            canvas.drawText("形勢",left,21*density,p);

            // A raw AI getter is not a solved-position evaluation. Keep the
            // metric explicit instead of claiming +2 means actual advantage.
            final String headline="完全解析 WDL/DTM";
            p.setTypeface(android.graphics.Typeface.DEFAULT);
            p.setTextSize(11f*density);
            p.setColor(snapshot.isEmpty()?Color.rgb(210,220,234):lastColor);
            canvas.drawText(headline,left+42*density,20*density,p);

            if (!snapshot.isEmpty()) {
                String value=signedScore(last);
                if (Math.abs((long)last)>FIXED_SCALE) value+=" *";
                p.setColor(Color.WHITE);
                p.setTypeface(android.graphics.Typeface.DEFAULT_BOLD);
                p.setTextSize(15f*density);
                canvas.drawText(value,right-p.measureText(value),40*density,p);
            }

            p.setStyle(Paint.Style.FILL);
            p.setColor(Color.argb(24,43,209,149));
            canvas.drawRect(left,top,right,mid,p);
            p.setColor(Color.argb(24,241,117,103));
            canvas.drawRect(left,mid,right,bottom,p);

            p.setColor(Color.rgb(121,136,157));
            p.setStyle(Paint.Style.STROKE);
            p.setStrokeWidth(density);
            canvas.drawRect(left,top,right,bottom,p);
            canvas.drawLine(left,mid,right,mid,p);
            p.setStyle(Paint.Style.FILL);

            p.setTypeface(android.graphics.Typeface.DEFAULT);
            p.setTextSize(9f*density);
            p.setColor(Color.rgb(156,174,194));
            canvas.drawText("自分側",left+4*density,top+12*density,p);
            canvas.drawText("0",left+4*density,mid-4*density,p);
            canvas.drawText("相手側",left+4*density,bottom-4*density,p);
            // Use a small, numeric label for each hard graph boundary.
            p.setTextSize(9f*density);
            p.setColor(Color.rgb(147,165,184));
            final String high="+2000";
            final String low="-2000";
            canvas.drawText(high,right-p.measureText(high)-5*density,
                    top+12*density,p);
            canvas.drawText(low,right-p.measureText(low)-5*density,
                    bottom-4*density,p);

            if (snapshot.isEmpty()) {
                String line;
                if (currentState==3) {
                    line="Hook失敗: "+statusText(detail)+" ["+detail+"]";
                } else if (currentState==2) {
                    line="監視中 ("+detail+") 着手:"+(activity[0]+activity[1])+
                            " 指令:"+activity[2]+" AI:"+activity[3];
                } else if (currentState==1) {
                    line="準備中: "+statusText(detail)+" ["+detail+"]";
                } else {
                    line="LSPosed Native接続待ち";
                }
                p.setColor(Color.LTGRAY);
                p.setTextSize(11f*density);
                if (p.measureText(line)>right-left) p.setTextSize(9f*density);
                canvas.drawText(line,left+6*density,mid-6*density,p);
                return;
            }

            // Draw line first; render a visible outlined circle for EVERY
            // available sample (not just the final one).
            p.setStyle(Paint.Style.STROKE);
            p.setStrokeWidth(2.3f*density);
            p.setStrokeCap(Paint.Cap.ROUND);
            float prevX=0,prevY=0;
            for(int i=0;i<snapshot.size();i++) {
                int v=snapshot.get(i);
                float x=xFor(i,snapshot.size()), y=yFor(v);
                // Missing turns are intentionally not linked as though the
                // module had observed an evaluation for every intermediate move.
                if(i>0 && snapshotMoveIndexes.get(i)>0 &&
                        snapshotMoveIndexes.get(i-1)==snapshotMoveIndexes.get(i)-1) {
                    p.setColor(scoreColor(v));
                    canvas.drawLine(prevX,prevY,x,y,p);
                }
                prevX=x;
                prevY=y;
            }
            for(int i=0;i<snapshot.size();i++) {
                final int v=snapshot.get(i);
                final float x=xFor(i,snapshot.size()), y=yFor(v);
                final int color=scoreColor(v);
                p.setStyle(Paint.Style.FILL);
                p.setColor(Color.rgb(21,26,37));
                canvas.drawCircle(x,y,4.5f*density,p);
                p.setStyle(Paint.Style.STROKE);
                p.setStrokeWidth(2.3f*density);
                p.setColor(color);
                canvas.drawCircle(x,y,4.3f*density,p);
            }

            // Selection is keyed by actual completed move number, not
            // an array index that changes on insertion or history trimming.
            int selectedIndex=-1;
            if(selectedEpoch==snapshotEpoch && selectedId>0) {
                selectedIndex=snapshotMoveIndexes.indexOf((int)selectedId);
            }
            if(selectedIndex>=0) drawSelection(canvas,selectedIndex);
            p.setStyle(Paint.Style.FILL);
        }

        private void drawSelection(Canvas canvas,int index) {
            final int raw=snapshot.get(index);
            final float x=xFor(index,snapshot.size()), y=yFor(raw);
            final int color=scoreColor(raw);

            p.setStyle(Paint.Style.STROKE);
            p.setColor(Color.WHITE);
            p.setStrokeWidth(1.7f*density);
            canvas.drawCircle(x,y,7.5f*density,p);
            p.setStyle(Paint.Style.FILL);

            final int moveId=snapshotMoveIndexes.get(index);
            final int game=snapshotGameScores.get(index);
            final String value = moveId > 0 ?
                    "#"+moveId+"  AI "+signedScore(raw) :
                    "手数未取得  AI "+signedScore(raw);
            // Show the unnormalized game-score getter only for diagnostics;
            // these values cannot be compared as if they were the same metric.
            final boolean hasGame = game != Integer.MIN_VALUE;
            final String sub = hasGame ? "盤面 raw "+game : "";
            p.setTypeface(android.graphics.Typeface.DEFAULT_BOLD);
            p.setTextSize(11f*density);
            final float firstWidth=p.measureText(value);
            p.setTypeface(android.graphics.Typeface.DEFAULT);
            p.setTextSize(9f*density);
            final float secondWidth=hasGame?p.measureText(sub):0f;
            final float bubbleW=Math.max(firstWidth,secondWidth)+20*density;
            final float bubbleH=(hasGame?39:25)*density;
            final float bubbleX=Math.max(4*density,
                    Math.min(getWidth()-bubbleW-4*density,x-bubbleW/2f));
            final float bubbleY=y-bubbleH-11*density>=4*density
                    ? y-bubbleH-11*density
                    : Math.min(getHeight()-bubbleH-4*density,y+11*density);
            r.set(bubbleX,bubbleY,bubbleX+bubbleW,bubbleY+bubbleH);
            p.setColor(Color.rgb(36,43,57));
            canvas.drawRoundRect(r,6*density,6*density,p);
            p.setStyle(Paint.Style.STROKE);
            p.setStrokeWidth(1.2f*density);
            p.setColor(color);
            canvas.drawRoundRect(r,6*density,6*density,p);
            p.setStyle(Paint.Style.FILL);
            p.setColor(Color.WHITE);
            p.setTypeface(android.graphics.Typeface.DEFAULT_BOLD);
            p.setTextSize(11f*density);
            canvas.drawText(value,bubbleX+10*density,bubbleY+16*density,p);
            if(hasGame){
                p.setTypeface(android.graphics.Typeface.DEFAULT);
                p.setTextSize(9f*density);
                p.setColor(Color.rgb(169,184,200));
                canvas.drawText(sub,bubbleX+10*density,bubbleY+30*density,p);
            }
        }

        private void selectNearPoint(float touchX,float touchY) {
            copyLatestSnapshot();
            int best=-1;
            float bestDistanceSq=(17*density)*(17*density);
            for(int i=0;i<snapshot.size();i++) {
                float dx=touchX-xFor(i,snapshot.size());
                float dy=touchY-yFor(snapshot.get(i));
                float dist=dx*dx+dy*dy;
                // Prefer the latest sample if multiple points overlap.
                if(dist<=bestDistanceSq) {
                    best=i;
                    bestDistanceSq=dist;
                }
            }
            selectedId=best<0?-1:snapshotMoveIndexes.get(best);
            selectedEpoch=snapshotEpoch;
            invalidate();
        }

        @Override public boolean onTouchEvent(MotionEvent e) {
            final Activity activity=active.get();
            if(activity==null)return false;
            switch(e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN: {
                    dragged=false;
                    downRawX=e.getRawX();
                    downRawY=e.getRawY();
                    FrameLayout.LayoutParams lp=(FrameLayout.LayoutParams)getLayoutParams();
                    startX=lp.leftMargin;
                    startY=lp.topMargin;
                    return true;
                }
                case MotionEvent.ACTION_MOVE: {
                    int dx=Math.round(e.getRawX()-downRawX);
                    int dy=Math.round(e.getRawY()-downRawY);
                    if(Math.abs(dx)>8*density || Math.abs(dy)>8*density)dragged=true;
                    if(!dragged)return true;
                    ViewGroup parent=(ViewGroup)getParent();
                    if(parent==null)return true;
                    FrameLayout.LayoutParams lp=(FrameLayout.LayoutParams)getLayoutParams();
                    lp.leftMargin=Math.max(0,Math.min(
                            Math.max(0,parent.getWidth()-getWidth()),startX+dx));
                    lp.topMargin=Math.max(0,Math.min(
                            Math.max(0,parent.getHeight()-getHeight()),startY+dy));
                    setLayoutParams(lp);
                    return true;
                }
                case MotionEvent.ACTION_UP: {
                    if(!dragged)selectNearPoint(e.getX(),e.getY());
                    FrameLayout.LayoutParams lp=(FrameLayout.LayoutParams)getLayoutParams();
                    offsetX=lp.leftMargin;
                    offsetY=lp.topMargin;
                    return true;
                }
                case MotionEvent.ACTION_CANCEL:return true;
                default:return false;
            }
        }
    }
}
