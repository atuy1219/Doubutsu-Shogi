// Modern LSPosed Native Hook API 102 and direct JNI delivery to the in-game View.
// Does not use sockets, overlay permission, foreground services, or Frida.
#include <jni.h>
#include <android/log.h>
#include <atomic>
#include <chrono>
#include <cmath>
#include <climits>
#include <cinttypes>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <dlfcn.h>
#include <pthread.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>

// Rust rules/position library: statically linked into libdobutsuhook.so.
extern "C" int32_t dobutsu_evaluate_dense(
    const uint8_t *,size_t,const int32_t *,const int32_t *,size_t,int32_t);

namespace {
constexpr char TAG[] = "DobutsuEval/Native";
using HookFun = int (*)(void *, void *, void **);
using UnhookFun = int (*)(void *);
using LoadedCallback = void (*)(const char *, void *);
struct NativeAPIEntries {
    uint32_t version;
    HookFun hook_func;
    UnhookFun unhook_func;
};
using ScoreGetter = int32_t (*)(void *, const void *);
using MoveGetter = uintptr_t (*)(void *, uintptr_t, const void *);
using ZeroGetter = uintptr_t (*)(void *, const void *);

std::atomic<JavaVM *> g_vm{nullptr};
jclass g_java_class = nullptr;
jmethodID g_score_callback = nullptr;
jmethodID g_status_callback = nullptr;
HookFun g_hook = nullptr;
std::atomic<bool> g_worker_started{false};
std::atomic<bool> g_hook_installed{false};
// Only a completed il2cpp_init may unlock IL2CPP metadata calls.
std::atomic<bool> g_runtime_ready{false};
std::atomic<bool> g_init_hooks_attempted{false};
std::atomic<void *> g_il2cpp_handle{nullptr};
using Il2cppInit = int (*)(const char *);
using Il2cppInitUtf16 = int (*)(const uint16_t *);
Il2cppInit g_init_original = nullptr;
Il2cppInitUtf16 g_init_utf16_original = nullptr;
void start_worker(void *handle);
std::atomic<int> g_source{-1};
std::atomic<int64_t> g_last_source_seen{0};
std::atomic<int64_t> g_last_emitted{0};
std::atomic<int32_t> g_last_value{INT32_MIN};
// Distinguish native bootstrap, Unity initialization and individual lookup failures.
std::atomic<int> g_stage{100};
std::atomic<bool> g_poller_started{false};
std::atomic<int> g_previous_logged_stage{-999};

struct Candidate {
    const char *klass;
    const char *method;
    ScoreGetter original;
    void *address;
    const void *method_info;
};
Candidate g_candidates[] = {
    {"ShogiModel",   "Get_AI_SCORE",   nullptr, nullptr, nullptr},
    {"ShogiControl", "Get_AI_Score",   nullptr, nullptr, nullptr},
    {"ShogiModel",   "Calc_SCORE",     nullptr, nullptr, nullptr},
    {"ShogiModel",   "Get_SCORE",      nullptr, nullptr, nullptr},
    {"ShogiModel",   "Get_My_SCORE",   nullptr, nullptr, nullptr},
};
constexpr int NCAND = sizeof(g_candidates) / sizeof(g_candidates[0]);
// The original score getters are not necessarily called by the game.
// Observe actual move processing, then query its own cached score with the
// correct ShogiModel instance and original MethodInfo. Data remains marked
// as a raw engine score: no win-probability or perfect-play claim.
struct GameEvent {
    const char *klass;
    const char *method;
    int params;
    void *original;
    void *address;
};
GameEvent g_events[] = {
    {"ShogiModel", "MakeMove", 1, nullptr, nullptr},
    {"ShogiModel", "v_MakeMove", 1, nullptr, nullptr},
    {"ShogiControl", "Cmd_MakeMove", 1, nullptr, nullptr},
    {"ShogiControl", "RunAI", 0, nullptr, nullptr},
};
constexpr int NEVENT = sizeof(g_events) / sizeof(g_events[0]);
std::atomic<void *> g_last_model{nullptr};
std::atomic<int> g_event_count[NEVENT];
std::atomic<int64_t> g_last_snapshot_ns{0};
std::atomic<int64_t> g_last_activity_update_ns{0};
std::atomic<bool> g_from_move{false};
// Only committed ShogiControl commands should create graph points. Search
// MakeMove() calls are not 1:1 with human/opponent moves.
std::atomic<int> g_committed_move_count{0};
// The game draws its turn label through player.drawTekazu(int). We use
// this observed label, rather than the number of native hook invocations.
// Never infer unique moves from identical scores or elapsed wall time.
std::atomic<int> g_ui_turn{0};
std::atomic<int> g_ui_turn_changes{0};
// This exact game binary uses the class static fields at mode +0x440 and
// turn cursor +0x448 to draw the on-screen counter. drawTekazu(int)'s
// argument is only a redraw flag, not a turn number.
// Binary: libil2cpp.so BuildId 408eff27a001571aac7d1c13edd8668dd4b6eccb.
std::atomic<uintptr_t> g_turn_binary_base{0};
using GetClassStaticData = void *(*)(void *);
GetClassStaticData g_get_static_data = nullptr;
// The UI announces *the next turn*, not necessarily a committed move.
// Hold a score observed before the next UI redraw until the redraw actually
// confirms that a new position was shown. Never synthesize a missing turn.
struct PendingEvaluation {
    int32_t ai;
    int32_t board;
    int before_turn;
    bool valid;
};
std::mutex g_pending_mutex;
PendingEvaluation g_pending{INT32_MIN,INT32_MIN,0,false};
using DrawTekazu = void (*)(void *, int32_t, const void *);
DrawTekazu g_original_draw_tekazu = nullptr;
// ShogiControl.model -> ShogiModel.Get_Ply() provides an independent
// game-model halfmove counter (if its runtime return type is numeric).
using FieldGetValue = void (*)(void *, const void *, void *);
FieldGetValue g_field_get_value = nullptr;
const void *g_model_field = nullptr;
void *g_ply_method = nullptr;
const void *g_ply_method_info = nullptr;
int g_ply_type = -1;
std::atomic<int> g_ply_to_ui_offset{INT32_MIN};
using ControlScoreGetter = int32_t (*)(void *, const void *);
ControlScoreGetter g_get_game_score = nullptr;
const void *g_get_game_score_info = nullptr;
jmethodID g_committed_callback = nullptr;
jmethodID g_activity_callback = nullptr;
jmethodID g_new_match_callback = nullptr;
jclass g_board_java_class = nullptr;
// The capture callback MUST receive a real player instance. An observed
// UnityMain crash (libil2cpp.so+0x14b5860, fault 0x20120018) was caused
// by treating a packed move argument (0x20120000) as an IL2CPP object.
void *g_player_class = nullptr;
jmethodID g_board_callback = nullptr;
jmethodID g_board_status_callback = nullptr;
jmethodID g_board_reset_callback = nullptr;
// The game's authoritative on-screen move number is produced by
// player.drawTekazu using ascPre static data. The game model provides
// the actual side to move, independently of GUI rendering. Never infer
// either from hook-call counts.
const void *g_asc_model_field=nullptr;
const void *g_asc_player_side_field=nullptr;
const void *g_shogi_turn_field=nullptr;
void *g_shogi_model_class=nullptr;
std::atomic<int> g_model_zero_side{-1};
int32_t g_cached_completed=-1;
int32_t g_cached_game_turn=-1;
// Keep a stable board for the *confirmed* previous game move. Draw paths
// can expose the new board while the UI still displays the old counter.
int32_t g_last_confirmed_board[12]{};
int32_t g_last_confirmed_move=-1;
bool g_last_confirmed_valid=false;

// Guard JNI delivery when multiple game draw routines run for one position.
std::mutex g_board_cache_mutex;
bool g_board_cache_valid = false;
bool g_board_cache_exact = false;
// The table stays mapped as shared read-only pages for the game process.
std::mutex g_table_mutex;
std::atomic<const uint8_t *> g_table_data{nullptr};
size_t g_table_size=0;
// 0: key-indexed DOBUTB03; 1: cohort-indexed DOBDENS1/DOBREF01.
// g_table_data is published only after header and opening-score validation.
std::string g_table_path="/data/user/0/jp.co.gmode.dobutsushogi/files/dobutsu.ref";
std::atomic<int64_t> g_last_table_attempt_ns{0};
int32_t g_board_cached[12]{};
int32_t g_hand_cached[64]{};
size_t g_hand_cached_length = 0;
// Extra redraw hooks are valid only when IL2CPP argument and return types
// match the corresponding ARM64 scalar function signature.
using RenderOne = void (*)(void *, uintptr_t, const void *);
using RenderTwo = void (*)(void *, uintptr_t, uintptr_t, const void *);
using RenderZero = void (*)(void *, const void *);
RenderOne g_draw_tekazu_original=nullptr;
RenderOne g_ban_reload_original=nullptr;
RenderOne g_ban_record_original=nullptr;
RenderTwo g_draw_board_original=nullptr;
RenderTwo g_move_piece_original=nullptr;
RenderZero g_init_battle_original=nullptr;

using DrawAll = void (*)(void *, int32_t, const void *);
DrawAll g_original_draw_all = nullptr;
using FindField = const void *(*)(void *, const char *);
using GetField = void (*)(void *, const void *, void *);
using FieldType = const void *(*)(const void *);
using GetType = int (*)(const void *);
using StaticFieldGet = void (*)(const void *, void *);
using ArrayLength = size_t (*)(const void *);
using ArrayByteLength = size_t (*)(const void *);
using ArrayHeader = uint32_t (*)();
using ObjectClass = void *(*)(void *);
using ArrayElementSize = uint32_t (*)(void *);
FindField g_find_field = nullptr;
GetField g_get_field = nullptr;
FieldType g_field_type = nullptr;
GetType g_type_code = nullptr;
StaticFieldGet g_static_field_get = nullptr;
ArrayLength g_array_len = nullptr;
ArrayByteLength g_array_bytes = nullptr;
ArrayHeader g_array_header = nullptr;
ObjectClass g_object_class = nullptr;
ArrayElementSize g_array_element_size = nullptr;
const void *g_board_field = nullptr;
const void *g_hand_field = nullptr;
int32_t g_piece_codes[8] = {
    INT32_MIN, INT32_MIN, INT32_MIN, INT32_MIN,
    INT32_MIN, INT32_MIN, INT32_MIN, INT32_MIN
};
std::atomic<int64_t> g_last_board_log{0};

std::atomic<int64_t> g_last_new_match_ns{0};
using ResetBattleVoid = void (*)(void *, const void *);
using ResetGameInt = int32_t (*)(void *, const void *);
struct MatchResetEntry {
    const char *klass;
    const char *method;
    int return_type;    // Il2CppTypeEnum: VOID=1, I4=8
    void *address;
    void *original;
};
MatchResetEntry g_match_reset[] = {
    {"player", "initBattle", 1, nullptr, nullptr},
    {"ShogiControl", "Cmd_Reset", 8, nullptr, nullptr}
};
constexpr int NRESET = sizeof(g_match_reset) / sizeof(g_match_reset[0]);

int64_t now_ns() {
    return std::chrono::duration_cast<std::chrono::nanoseconds>(
        std::chrono::steady_clock::now().time_since_epoch()).count();
}

template<class Callback>
void with_java(Callback cb) {
    JavaVM *vm = g_vm.load(std::memory_order_acquire);
    if (!vm || !g_java_class) return;
    JNIEnv *env = nullptr;
    bool attached = false;
    jint state = vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6);
    if (state == JNI_EDETACHED) {
        if (vm->AttachCurrentThread(&env, nullptr) != JNI_OK) return;
        attached = true;
    } else if (state != JNI_OK) return;
    cb(env);
    if (env->ExceptionCheck()) {
        env->ExceptionDescribe();
        env->ExceptionClear();
    }
    if (attached) vm->DetachCurrentThread();
}

void status(int state, int details) {
    __android_log_print(ANDROID_LOG_INFO, TAG, "status=%d detail=%d", state, details);
    if (g_status_callback)
        with_java([&](JNIEnv *env) {
            env->CallStaticVoidMethod(g_java_class, g_status_callback, state, details);
        });
}

void stage(int code, const char *reason) {
    g_stage.store(code, std::memory_order_relaxed);
    if (g_previous_logged_stage.exchange(code) != code) {
        __android_log_print(ANDROID_LOG_INFO, TAG,
            "phase=%d: %s", code, reason);
        status(1, code);
    }
}

void notify_new_match(int trigger) {
    const int64_t t = now_ns();
    // Both reset entry points can fire for the very same new game.
    // Only clear once, and avoid a rapid second clear after the first move.
    int64_t previous = g_last_new_match_ns.load(std::memory_order_relaxed);
    if (t - previous < 2'000'000'000LL) return;
    g_last_new_match_ns.store(t, std::memory_order_relaxed);
    g_from_move.store(true, std::memory_order_release);
    g_last_model.store(nullptr, std::memory_order_release);
    g_last_snapshot_ns.store(0, std::memory_order_relaxed);
    g_last_value.store(INT32_MIN, std::memory_order_relaxed);
    g_last_emitted.store(0, std::memory_order_relaxed);
    g_source.store(-1, std::memory_order_relaxed);
    g_last_activity_update_ns.store(0, std::memory_order_relaxed);
    g_committed_move_count.store(0, std::memory_order_relaxed);
    g_ui_turn.store(0, std::memory_order_relaxed);
    g_ui_turn_changes.store(0, std::memory_order_relaxed);
    {
        std::lock_guard<std::mutex> lock(g_pending_mutex);
        g_pending.valid=false;
    }
    g_ply_to_ui_offset.store(INT32_MIN,std::memory_order_relaxed);
    for (auto &counter : g_event_count) counter.store(0, std::memory_order_relaxed);
    __android_log_print(ANDROID_LOG_INFO, TAG,
        "New match/reset detected via %s.%s; clearing score history",
        g_match_reset[trigger].klass, g_match_reset[trigger].method);
    if (g_new_match_callback) with_java([&](JNIEnv *env) {
        env->CallStaticVoidMethod(g_java_class, g_new_match_callback, trigger);
    });
}

void reset_by_init_battle(void *self, const void *method) {
    const auto original = reinterpret_cast<ResetBattleVoid>(g_match_reset[0].original);
    if (!original) return;
    // Clear BEFORE the initializer redraws turn 1. Clearing afterwards
    // would erase the new player.drawTekazu(1) observation.
    notify_new_match(0);
    original(self, method);
}

int32_t reset_by_cmd_reset(void *self, const void *method) {
    const auto original = reinterpret_cast<ResetGameInt>(g_match_reset[1].original);
    if (!original) return 0;
    notify_new_match(1);
    return original(self, method);
}
void *g_reset_replacements[NRESET] = {
    reinterpret_cast<void *>(reset_by_init_battle),
    reinterpret_cast<void *>(reset_by_cmd_reset)
};

void emit_activity(int which) {
    const int count = g_event_count[which].fetch_add(1, std::memory_order_relaxed) + 1;
    int64_t t = now_ns();
    if (count == 1 || t - g_last_activity_update_ns.exchange(t) >= 400'000'000LL) {
        __android_log_print(ANDROID_LOG_DEBUG, TAG, 
            "Event %s.%s called count=%d", g_events[which].klass,
            g_events[which].method, count);
        if (g_activity_callback) with_java([&](JNIEnv *env) {
            env->CallStaticVoidMethod(g_java_class, g_activity_callback, which, count);
        });
    }
}
void snapshot_model(void *self, int which) {
    if (!self) return;
    // Use a caller-owned live ShogiModel after a real move. Do not execute
    // Calc_SCORE or any modifying AI/search method.
    int64_t t = now_ns();
    if (t - g_last_snapshot_ns.load(std::memory_order_relaxed) < 250'000'000LL)
        return;
    const auto &getter = g_candidates[3]; // Get_SCORE() -> int
    const auto &fallback = g_candidates[0]; // Get_AI_SCORE() -> int
    const Candidate *selected = getter.original ? &getter :
                                fallback.original ? &fallback : nullptr;
    if (!selected || !selected->method_info) {
        __android_log_print(ANDROID_LOG_WARN,TAG,
            "Move observed but both Get_SCORE/Get_AI_SCORE trampolines missing");
        return;
    }
    const auto previous = g_last_snapshot_ns.exchange(t, std::memory_order_relaxed);
    (void)previous;
    const int32_t value = selected->original(self, selected->method_info);
    if (value < -100000000 || value > 100000000) {
        __android_log_print(ANDROID_LOG_WARN,TAG,
            "Out-of-range score after move, source=%s value=%d",selected->method,value);
        return;
    }
    g_from_move.store(true, std::memory_order_release);
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Score snapshot after %s: %s=%d",
        g_events[which].method,selected->method,value);
    if (g_score_callback) with_java([&](JNIEnv *env) {
        // source=5 means score was explicitly sampled after a game move.
        env->CallStaticVoidMethod(g_java_class,g_score_callback,value,5);
    });
}
uintptr_t observe_make_move(void *self, uintptr_t arg, const void *method) {
    const auto original = reinterpret_cast<MoveGetter>(g_events[0].original);
    if (!original) return 0;
    const uintptr_t result = original(self,arg,method);
    g_last_model.store(self,std::memory_order_release);
    emit_activity(0);
    // This may run during speculative AI search: do not add a graph point.
    return result;
}
uintptr_t observe_v_make_move(void *self, uintptr_t arg, const void *method) {
    const auto original = reinterpret_cast<MoveGetter>(g_events[1].original);
    if (!original) return 0;
    const uintptr_t result = original(self,arg,method);
    g_last_model.store(self,std::memory_order_release);
    emit_activity(1);
    // This may run during speculative AI search: do not add a graph point.
    return result;
}
// This function is called from the game's own UI turn-rendering path.
// Value is captured after the original draw routine, but it is NOT a
// substitute for reverse-engineering the actual ShogiModel history count.
// Report only confirmed (game-drawn) completed moves. An absent UI turn
// results in no point rather than a fabricated "move #N".
void send_confirmed_turn(int32_t ai,int32_t board,int nextTurn,
        const char *reason) {
    const int completed=nextTurn-1;
    if(completed<=0 || ai==INT32_MIN || !g_committed_callback) return;
    g_from_move.store(true,std::memory_order_release);
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Confirmed completed move #%d by %s: AI=%d GAME=%d",
        completed,reason,ai,board);
    with_java([&](JNIEnv *env) {
        env->CallStaticVoidMethod(g_java_class,g_committed_callback,
            ai,completed,board);
    });
}

// This hook observes exactly the counter drawn by the game's UI.
bool readable_game_memory(const void *pointer, size_t length) {
    uintptr_t address=reinterpret_cast<uintptr_t>(pointer);
    if(!address || length>4096 || address+length<address) return false;
    FILE *maps=std::fopen("/proc/self/maps","r");
    if(!maps) return false;
    char line[1024]{};
    bool valid=false;
    while(std::fgets(line,sizeof(line),maps)) {
        uintptr_t start=0,end=0;
        char permissions[5]{};
        if(std::sscanf(line,"%" SCNxPTR "-%" SCNxPTR " %4s",
             &start,&end,permissions)==3 && permissions[0]=='r' &&
             address>=start && address+length<=end) {
            valid=true;
            break;
        }
    }
    std::fclose(maps);
    return valid;
}

int read_actual_drawn_turn() {
    const uintptr_t base=g_turn_binary_base.load(std::memory_order_acquire);
    if(!base || !g_get_static_data) return 0;
    // Generated player.drawTekazu reads its Il2CppClass from
    // base+0x2edc340 (GOT slot at base+0x2e60808), then the
    // class static data at +0x440 and +0x448.
    const uintptr_t class_slot=base+0x2edc340;
    if(!readable_game_memory(reinterpret_cast<void *>(class_slot),8))
        return 0;
    void *klass=nullptr;
    std::memcpy(&klass,reinterpret_cast<const void *>(class_slot),8);
    if(!klass || !readable_game_memory(klass,0xe8)) return 0;
    void *statics=g_get_static_data(klass);
    if(!statics || !readable_game_memory(statics,0x44c)) return 0;
    int32_t mode=0,cursor=0;
    auto *bytes=static_cast<const unsigned char *>(statics);
    std::memcpy(&mode,bytes+0x440,4);
    std::memcpy(&cursor,bytes+0x448,4);
    int64_t drawn=(int64_t)cursor+(mode==3?0:1);
    if(drawn<1 || drawn>10000) return 0;
    __android_log_print(ANDROID_LOG_DEBUG,TAG,
       "Real drawTekazu: mode=%d cursor=%d displayed=%" PRId64,
       mode,cursor,drawn);
    return (int)drawn;
}

void observe_draw_tekazu(void *self,int32_t redrawOption,const void *method) {
    const DrawTekazu fn=g_original_draw_tekazu;
    if(!fn) return;
    fn(self,redrawOption,method);
    const int displayed=read_actual_drawn_turn();
    if(displayed<=0) {
        __android_log_print(ANDROID_LOG_WARN,TAG,
            "drawTekazu could not decode the actual displayed turn");
        return;
    }
    const int before=g_ui_turn.exchange(displayed,std::memory_order_acq_rel);
    if(before==displayed) return;
    const int updates=g_ui_turn_changes.fetch_add(1,std::memory_order_relaxed)+1;
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Displayed turn updated: %d -> %d (event=%d)",
        before,displayed,updates);
    PendingEvaluation pending{INT32_MIN,INT32_MIN,0,false};
    {
        std::lock_guard<std::mutex> lock(g_pending_mutex);
        if(g_pending.valid && displayed>g_pending.before_turn) {
            pending=g_pending;
            g_pending.valid=false;
        }
    }
    if(pending.valid)
        send_confirmed_turn(pending.ai,pending.board,displayed,
            "drawTekazu-decoded-counter");
}

uintptr_t observe_cmd_make_move(void *self,uintptr_t arg,const void *method) {
    const auto original=reinterpret_cast<MoveGetter>(g_events[2].original);
    if(!original)return 0;
    const int uiBefore=g_ui_turn.load(std::memory_order_acquire);
    const uintptr_t result=original(self,arg,method);
    const int uiAfter=g_ui_turn.load(std::memory_order_acquire);
    emit_activity(2);
    const int invocation=g_committed_move_count.fetch_add(
        1,std::memory_order_relaxed)+1;
    const Candidate &ai=g_candidates[1];
    int32_t aiScore=INT32_MIN,gameScore=INT32_MIN;
    if(ai.original && ai.method_info && self)
        aiScore=ai.original(self,ai.method_info);
    if(g_get_game_score && g_get_game_score_info && self)
        gameScore=g_get_game_score(self,g_get_game_score_info);

    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Cmd_MakeMove call=%d uiBefore=%d uiAfter=%d arg=0x%" PRIxPTR
        " AI=%d GAME=%d result=%" PRIuPTR,
        invocation,uiBefore,uiAfter,arg,aiScore,gameScore,result);

    if(aiScore==INT32_MIN) return result;
    if(uiAfter>1 && uiAfter>uiBefore) {
        // The original command synchronously advanced the visible UI turn.
        send_confirmed_turn(aiScore,gameScore,uiAfter,"Cmd-after-UI-change");
    } else {
        // The UI can redraw AFTER Cmd_MakeMove returns (or the command may
        // run twice for one move). Replace the pending sample and emit only
        // if/when drawTekazu confirms a new displayed turn.
        std::lock_guard<std::mutex> lock(g_pending_mutex);
        g_pending={aiScore,gameScore,uiAfter>0?uiAfter:uiBefore,true};
        __android_log_print(ANDROID_LOG_INFO,TAG,
            "Cmd_MakeMove pending UI confirmation, before=%d after=%d",
            uiBefore,uiAfter);
    }
    return result;
}

uintptr_t observe_run_ai(void *self, const void *method) {
    const auto original = reinterpret_cast<ZeroGetter>(g_events[3].original);
    if (!original) return 0;
    emit_activity(3);
    return original(self,method);
}
void *g_event_replacements[NEVENT] = {
    reinterpret_cast<void *>(observe_make_move),
    reinterpret_cast<void *>(observe_v_make_move),
    reinterpret_cast<void *>(observe_cmd_make_move),
    reinterpret_cast<void *>(observe_run_ai),
};

void emit_score(int32_t value, int which) {
    if (value < -100000000 || value > 100000000) return;
    // Once live move snapshots have been observed, avoid mixing this stream
    // with unsolicited getter calls from other internal searches.
    if (g_from_move.load(std::memory_order_acquire)) return;
    int64_t t = now_ns();
    int current = g_source.load(std::memory_order_relaxed);
    int64_t seen = g_last_source_seen.load(std::memory_order_relaxed);
    // Prefer Get_AI_SCORE if called, but fall back to a lower-priority
    // evaluation function when the preferred getter has been idle for 5 s.
    if (current >= 0 && which > current && t - seen < 5'000'000'000LL) return;
    if (current != which) {
        g_source.store(which, std::memory_order_relaxed);
        g_last_value.store(INT32_MIN, std::memory_order_relaxed);
        g_last_emitted.store(0, std::memory_order_relaxed);
        __android_log_print(ANDROID_LOG_INFO, TAG,
            "Active score source: %s.%s", g_candidates[which].klass, g_candidates[which].method);
    }
    g_last_source_seen.store(t, std::memory_order_relaxed);
    if (t - g_last_emitted.load(std::memory_order_relaxed) < 300'000'000LL) return;
    if (g_last_value.load(std::memory_order_relaxed) == value &&
        t - g_last_emitted.load(std::memory_order_relaxed) < 3'000'000'000LL) return;
    g_last_emitted.store(t, std::memory_order_relaxed);
    g_last_value.store(value, std::memory_order_relaxed);
    if (g_score_callback)
        with_java([&](JNIEnv *env) {
            env->CallStaticVoidMethod(g_java_class, g_score_callback, value, which);
        });
}

#define DEFINE_HOOK(I) \
int32_t hook_##I(void *self, const void *method) { \
    const auto fn = g_candidates[I].original; \
    if (!fn) return 0; \
    const int32_t value = fn(self, method); \
    return value; \
}
DEFINE_HOOK(0)
DEFINE_HOOK(1)
DEFINE_HOOK(2)
DEFINE_HOOK(3)
DEFINE_HOOK(4)
#undef DEFINE_HOOK

void *g_replacements[] = {
    reinterpret_cast<void *>(hook_0),
    reinterpret_cast<void *>(hook_1),
    reinterpret_cast<void *>(hook_2),
    reinterpret_cast<void *>(hook_3),
    reinterpret_cast<void *>(hook_4),
};

bool is_executable_il2cpp_address(void *address) {
    if (!address) return false;
    FILE *f = std::fopen("/proc/self/maps", "r");
    if (!f) return false;
    char line[1024]{};
    bool found = false;
    uintptr_t needle = reinterpret_cast<uintptr_t>(address);
    while (std::fgets(line, sizeof(line), f)) {
        uintptr_t start = 0, end = 0;
        char perms[5]{};
        if (std::sscanf(line, "%" SCNxPTR "-%" SCNxPTR " %4s",
                        &start, &end, perms) != 3) continue;
        if (start <= needle && needle < end && perms[2] == 'x'
                && std::strstr(line, "libil2cpp.so")) {
            found = true;
            break;
        }
    }
    std::fclose(f);
    return found;
}

template<typename F>
F symbol(void *handle, const char *name) {
    return reinterpret_cast<F>(dlsym(handle, name));
}

// Snapshot only after the game's real board-rendering routine. An AI search
// may mutate ShogiModel thousands of times: these changes are deliberately
// NOT treated as actual moves.
// Only enable counter decoding for the exact game's ARM64
// drawTekazu implementation whose disassembly we verified.
void init_verified_turn_reader(void *handle, void *image, void *player_class,
    const void *(*method_find)(void *,const char *,int),
    void *(*class_find)(void *,const char *,const char *)) {
    g_get_static_data=symbol<GetClassStaticData>(
        handle,"il2cpp_class_get_static_field_data");
    const void *info=method_find(player_class,"drawTekazu",1);
    void *entry=nullptr;
    if(info)std::memcpy(&entry,info,sizeof(entry));
    Dl_info details{};
    const bool mapped=entry && dladdr(entry,&details)!=0 && details.dli_fbase;
    const uintptr_t base=mapped?
        reinterpret_cast<uintptr_t>(details.dli_fbase):0;
    constexpr uint8_t known_prologue[]={
        0xff,0x03,0x01,0xd1,0xfe,0x0b,0x00,0xf9,
        0xf6,0x57,0x02,0xa9,0xf4,0x4f,0x03,0xa9,
        0xb3,0xd2,0x00,0xd0,0x96,0xc6,0x00,0xf0,
        0xf5,0x03,0x01,0x2a,0x68,0x06,0x6c,0x39
    };
    const bool verified=base && g_get_static_data &&
        reinterpret_cast<uintptr_t>(entry)-base==0x158d7c8 &&
        is_executable_il2cpp_address(entry) &&
        std::memcmp(entry,known_prologue,sizeof(known_prologue))==0;
    if(verified) {
        g_turn_binary_base.store(base,std::memory_order_release);
        __android_log_print(ANDROID_LOG_INFO,TAG,
            "Verified player.drawTekazu native counter RVA 0x158d7c8");
    }else{
        __android_log_print(ANDROID_LOG_WARN,TAG,
            "UI turn counter disabled: target binary signature mismatch");
    }
    if(!g_find_field || !g_field_type || !g_type_code ||
       !g_static_field_get || !g_get_field)return;
    void *asc=class_find(image,"","ascPre");
    void *modelClass=class_find(image,"","ShogiModel");
    if(!asc || !modelClass)return;
    const void *model=g_find_field(asc,"mShogiModel");
    const void *playerSide=g_find_field(asc,"plSide");
    const void *turn=g_find_field(modelClass,"turn");
    const auto code=[&](const void *f)->int {
        const void *t=f?g_field_type(f):nullptr;
        return t?g_type_code(t):-1;
    };
    // An int32 model.turn is independent of ShogiModel.MakeMove's
    // search-local simulations and can be read without calling any getter.
    if(code(turn)==8 && (code(model)==18 || code(model)==17)) {
        g_asc_model_field=model;
        g_shogi_turn_field=turn;
        g_shogi_model_class=modelClass;
    }
    if(code(playerSide)==8)g_asc_player_side_field=playerSide;
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Model turn field ready=%d UI counter ready=%d",
        (int)(g_shogi_turn_field!=nullptr),(int)verified);
}

int read_game_model_turn() {
    if(!g_asc_model_field || !g_shogi_turn_field ||
       !g_static_field_get || !g_get_field || !g_object_class) return -1;
    void *model=nullptr;
    g_static_field_get(g_asc_model_field,&model);
    if(!model || !readable_game_memory(model,sizeof(void *)))return -1;
    void *klass=nullptr;
    std::memcpy(&klass,model,sizeof(klass));
    if(klass!=g_shogi_model_class)return -1;
    int32_t turn=-1;
    g_get_field(model,g_shogi_turn_field,&turn);
    // Do not guess whether turn is 0/1 or 1/2 when the exact
    // representation has not been witnessed in this game's runtime.
    return turn==0||turn==1?turn:-1;
}

// Under normal play, the model's side-to-move is opposite to the
// owner of the just committed move. Calibrate 0/1 to rendered piece
// ownership ONLY when both a real board delta and an advancing UI
// counter have been verified; do not assume model 0 is always human.
int identify_mover(const int32_t *oldBoard,const int32_t *newBoard) {
    int side=-1;
    for(int i=0;i<12;i++) {
        int oldPiece=oldBoard[i],newPiece=newBoard[i];
        if(oldPiece>=0 && newPiece==-1) {
            int s=(oldPiece&0x20)?1:0;
            if(side!=-1 && side!=s)return -1;
            side=s;
        }
    }
    if(side!=-1)return side;
    // A drop has no departing board piece. Identify the entering piece.
    for(int i=0;i<12;i++)if(oldBoard[i]==-1 && newBoard[i]>=0){
        int s=(newBoard[i]&0x20)?1:0;
        if(side!=-1 && side!=s)return -1;
        side=s;
    }
    return side;
}

void board_status(int state,int reason) {
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Board reader status=%d reason=%d",state,reason);
    if(g_board_java_class && g_board_status_callback)
        with_java([&](JNIEnv *env) {
            env->CallStaticVoidMethod(g_board_java_class,
                g_board_status_callback,state,reason);
        });
}

bool read_int_array(void *owner,const void *field,int32_t *out,
                    size_t capacity,size_t *size) {
    if(!owner || !field || !g_get_field || !g_array_len ||
       !g_array_bytes || !g_array_header || !g_object_class ||
       !g_array_element_size) return false;
    void *object=nullptr;
    g_get_field(owner,field,&object);
    // The old implementation passed 0x20120000 into il2cpp_array_length,
    // which unconditionally loads [object + 0x18]. Guard the actual array
    // pointer before invoking any IL2CPP array API.
    if(!object || !readable_game_memory(object,0x20)) return false;
    const size_t length=g_array_len(object);
    // A valid hands array can be empty; boardData is checked for length 12
    // separately. Reject only out-of-range lengths and invalid pointers.
    if(length>capacity || length>64) return false;
    const size_t bytes=g_array_bytes(object);
    if(bytes!=length*sizeof(int32_t))return false;
    if(g_array_element_size(g_object_class(object))!=sizeof(int32_t))
        return false;
    const uint32_t header=g_array_header();
    if(header<24 || header>64)return false;
    if(!readable_game_memory(object,header+bytes))return false;
    // Both SZARRAY and MDARRAY of int32 have contiguous data following
    // the IL2CPP array-object header; bounds are managed separately.
    std::memcpy(out,static_cast<const uint8_t *>(object)+header,bytes);
    *size=length;
    return true;
}

// The bundled, exhaustively verified C reference table is mapped read-only.
bool load_verified_tablebase() {
    if(g_table_data.load(std::memory_order_acquire))return true;
    const int64_t current=now_ns();
    if(current-g_last_table_attempt_ns.load(std::memory_order_acquire)
       <5'000'000'000LL)return false;
    std::lock_guard<std::mutex> lock(g_table_mutex);
    if(g_table_data.load(std::memory_order_acquire))return true;
    if(current-g_last_table_attempt_ns.load(std::memory_order_acquire)
       <5'000'000'000LL)return false;
    g_last_table_attempt_ns.store(current,std::memory_order_release);

    const int fd=::open(g_table_path.c_str(),O_RDONLY|O_CLOEXEC);
    if(fd<0)return false;
    struct stat st{};
    constexpr uint64_t EXPECTED_BYTES=167527978ULL;
    constexpr uint64_t EXPECTED_COUNT=167527962ULL;
    if(::fstat(fd,&st)!=0 || !S_ISREG(st.st_mode) ||
       static_cast<uint64_t>(st.st_size)!=EXPECTED_BYTES) {
        ::close(fd);
        __android_log_print(ANDROID_LOG_ERROR,TAG,
            "Bundled exact tablebase has invalid size: %s",g_table_path.c_str());
        return false;
    }
    const size_t bytes=static_cast<size_t>(st.st_size);
    void *address=::mmap(nullptr,bytes,PROT_READ,MAP_PRIVATE,fd,0);
    ::close(fd);
    if(address==MAP_FAILED)return false;
    const auto *data=static_cast<const uint8_t *>(address);
    uint64_t count=0;
    std::memcpy(&count,data+8,8);
    const bool valid=std::memcmp(data,"DOBREF01",8)==0 &&
                     count==EXPECTED_COUNT && bytes==16ULL+count;
    if(!valid) {
        ::munmap(address,bytes);
        __android_log_print(ANDROID_LOG_ERROR,TAG,
            "Bundled exact tablebase rejected: invalid header or count");
        return false;
    }
    const int32_t opening[12]={
        0x23,0x21,0x25,-1,0x27,-1,-1,0x06,-1,0x04,0x00,0x02
    };
    const int32_t score=dobutsu_evaluate_dense(data,bytes,opening,nullptr,0,0);
    if(score!=-1922) {
        ::munmap(address,bytes);
        __android_log_print(ANDROID_LOG_ERROR,TAG,
            "Bundled exact tablebase opening mismatch: %d",score);
        return false;
    }
    g_table_size=bytes;
    ::madvise(address,bytes,MADV_RANDOM);
    g_table_data.store(data,std::memory_order_release);
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Bundled exact WDL/DTM table mapped: bytes=%zu",bytes);
    return true;
}

void snapshot_board(void *self) {
    if(!self || !g_board_java_class || !g_board_callback ||
       !g_player_class)return;
    // A method's declared class and scalar parameter types alone do not
    // prove its first machine register is "player *". Check the managed
    // object header and refuse unknown/malformed owners before field API.
    if(!readable_game_memory(self,sizeof(void *))) return;
    void *actualClass=nullptr;
    std::memcpy(&actualClass,self,sizeof(actualClass));
    if(actualClass!=g_player_class) {
        int64_t t=now_ns();
        if(t-g_last_board_log.exchange(t)>3'000'000'000LL)
            __android_log_print(ANDROID_LOG_WARN,TAG,
                "Skipping board snapshot from non-player arg: self=%p class=%p expected=%p",
                self,actualClass,g_player_class);
        return;
    }
    int32_t board[64]{};
    int32_t hands[64]{};
    size_t boardN=0, handN=0;
    if(!read_int_array(self,g_board_field,board,64,&boardN)) {
        int64_t t=now_ns();
        if(t-g_last_board_log.exchange(t)>5'000'000'000LL)
            __android_log_print(ANDROID_LOG_WARN,TAG,
                "boardData array unavailable or not int32[]");
        return;
    }
    // The UI expects a 3x4 array. Do not fabricate a 12-cell board if
    // the game's field is an unexpected length/layout.
    if(boardN!=12){
        int64_t t=now_ns();
        if(t-g_last_board_log.exchange(t)>5'000'000'000LL)
            __android_log_print(ANDROID_LOG_WARN,TAG,
                "boardData length=%zu; expected 12; hold snapshot",boardN);
        board_status(3,261);
        return;
    }
    bool handOk=read_int_array(self,g_hand_field,hands,64,&handN);
    // A game position contains hands even when they are empty, represented
    // as a real IL2CPP array with unused -1 slots. Never score with missing
    // hand data or an all-lions uninitialized board.
    if(!handOk) {
        int64_t t=now_ns();
        if(t-g_last_board_log.exchange(t)>5'000'000'000LL)
            __android_log_print(ANDROID_LOG_WARN,TAG,
                "Holding mini-board: motiData not ready");
        return;
    }
    const int displayedNext=read_actual_drawn_turn();
    const int completed=displayedNext>0?displayedNext-1:-1;
    const int modelTurn=read_game_model_turn();
    {
        std::lock_guard<std::mutex> lock(g_board_cache_mutex);
        if(completed>=0) {
            if(g_last_confirmed_valid &&
               completed==g_last_confirmed_move+1 && modelTurn>=0) {
                const int mover=identify_mover(g_last_confirmed_board,board);
                if(mover>=0) {
                    const int nextOwner=1-mover;
                    const int calibrated=modelTurn==0?nextOwner:1-nextOwner;
                    const int old=g_model_zero_side.load(std::memory_order_acquire);
                    if(old==-1) {
                        g_model_zero_side.store(calibrated,std::memory_order_release);
                        __android_log_print(ANDROID_LOG_INFO,TAG,
                            "Calibrated ShogiModel.turn: model0 side=%d",
                            calibrated);
                    } else if(old>=0 && old!=calibrated) {
                        g_model_zero_side.store(-2,std::memory_order_release);
                        __android_log_print(ANDROID_LOG_ERROR,TAG,
                            "Inconsistent model turn encoding: exact TB disabled");
                    }
                }
            }
            if(!g_last_confirmed_valid ||
               completed>g_last_confirmed_move) {
                std::memcpy(g_last_confirmed_board,board,sizeof(g_last_confirmed_board));
                g_last_confirmed_move=completed;
                g_last_confirmed_valid=true;
            }
        }
    }
    const int zeroSide=g_model_zero_side.load(std::memory_order_acquire);
    const int sideToMove=(modelTurn>=0 && zeroSide>=0)?
        (modelTurn==0?zeroSide:1-zeroSide):-1;
    // No fallback is permitted: an unknown side, unavailable table, or
    // position without an exact tablebase result produces no graph sample.
    if(sideToMove<0 || !load_verified_tablebase())return;
    const uint8_t *tb=g_table_data.load(std::memory_order_acquire);
    const int32_t evaluatedScore=dobutsu_evaluate_dense(
        tb,g_table_size,board,hands,handN,sideToMove);
    if(evaluatedScore==INT32_MIN)return;
    constexpr bool exact=true;
    {
        std::lock_guard<std::mutex> lock(g_board_cache_mutex);
        if(g_board_cache_valid && g_hand_cached_length==handN &&
            g_board_cache_exact==exact &&
            g_cached_completed==completed &&
            g_cached_game_turn==sideToMove &&
            std::memcmp(g_board_cached,board,12*sizeof(int32_t))==0 &&
            (handN==0 || std::memcmp(g_hand_cached,hands,
                                    handN*sizeof(int32_t))==0)) {
            return;
        }
        std::memcpy(g_board_cached,board,12*sizeof(int32_t));
        g_hand_cached_length=handN;
        if(handN)std::memcpy(g_hand_cached,hands,handN*sizeof(int32_t));
        g_board_cache_valid=true;
        g_board_cache_exact=exact;
        g_cached_completed=completed;
        g_cached_game_turn=sideToMove;
    }
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Board snapshot move=%d model=%d side=%d score=%d exact=%d",
        completed,modelTurn,sideToMove,evaluatedScore,(int)exact);
    with_java([&](JNIEnv *env) {
        jintArray jb=env->NewIntArray(12);
        jintArray jh=env->NewIntArray(handOk?(jsize)handN:0);
        jintArray jc=env->NewIntArray(8);
        if(!jb || !jh || !jc) {
            if(jb)env->DeleteLocalRef(jb);
            if(jh)env->DeleteLocalRef(jh);
            if(jc)env->DeleteLocalRef(jc);
            return;
        }
        env->SetIntArrayRegion(jb,0,12,board);
        if(handOk)env->SetIntArrayRegion(jh,0,(jsize)handN,hands);
        env->SetIntArrayRegion(jc,0,8,g_piece_codes);
        env->CallStaticVoidMethod(g_board_java_class,g_board_callback,
            jb,jh,jc,(jint)evaluatedScore,
            (jint)completed,(jint)sideToMove,(jint)(exact?1:0));
        env->DeleteLocalRef(jb);
        env->DeleteLocalRef(jh);
        env->DeleteLocalRef(jc);
    });
}

// Each hook is installed independently. All take a *player* instance
// whose boardData and motiData fields are already used by the game UI.
void on_draw_tekazu_board(void *self,uintptr_t option,const void *method) {
    RenderOne old=g_draw_tekazu_original;
    if(!old)return;
    old(self,option,method);
    snapshot_board(self); // snapshot method is shared below
}
void on_ban_reload_board(void *self,uintptr_t option,const void *method) {
    RenderOne old=g_ban_reload_original;
    if(!old)return;
    old(self,option,method);
    snapshot_board(self);
}
void on_ban_record_board(void *self,uintptr_t option,const void *method) {
    RenderOne old=g_ban_record_original;
    if(!old)return;
    old(self,option,method);
    snapshot_board(self);
}
void on_draw_board_koma(void *self,uintptr_t a,uintptr_t b,const void *method) {
    RenderTwo old=g_draw_board_original;
    if(!old)return;
    old(self,a,b,method);
    snapshot_board(self);
}
void on_move_koma_pos(void *self,uintptr_t a,uintptr_t b,const void *method) {
    RenderTwo old=g_move_piece_original;
    if(!old)return;
    old(self,a,b,method);
    snapshot_board(self);
}
void on_init_battle_board(void *self,const void *method) {
    RenderZero old=g_init_battle_original;
    if(!old)return;
    {
        std::lock_guard<std::mutex> lock(g_board_cache_mutex);
        g_board_cache_valid=false;
        g_board_cache_exact=false;
        g_cached_completed=-1;
        g_cached_game_turn=-1;
        g_model_zero_side.store(-1,std::memory_order_release);
        g_last_confirmed_valid=false;
        g_last_confirmed_move=-1;
    }
    if(g_board_reset_callback)with_java([&](JNIEnv *env){
        env->CallStaticVoidMethod(g_board_java_class,g_board_reset_callback);
    });
    old(self,method);
    snapshot_board(self);
}

bool install_extra_board_redraw_hooks(
    void *image, void *klass,
    const void *(*method_find)(void*,const char*,int),
    const void *(*get_return)(const void*),
    const void *(*get_param)(const void*,uint32_t),
    int (*type_code)(const void*)) {
    if(!klass || !g_hook || !get_return || !get_param || !type_code) return false;
    struct Entry {
        const char *name;
        int parameters;
        void *replacement;
        void **original;
    };
    // The two-argument moveKomaPos adapter is the confirmed crash source.
    // It sometimes receives a packed move value as x0, NOT a player*.
    // Keep only verified instance-level UI redraw methods until their
    // actual signatures are independently established by disassembly.
    Entry hooks[] = {
        {"drawTekazu",1,reinterpret_cast<void *>(on_draw_tekazu_board),
            reinterpret_cast<void **>(&g_draw_tekazu_original)},
        {"banReload",1,reinterpret_cast<void *>(on_ban_reload_board),
            reinterpret_cast<void **>(&g_ban_reload_original)},
        {"banRecord",1,reinterpret_cast<void *>(on_ban_record_board),
            reinterpret_cast<void **>(&g_ban_record_original)},
        {"initBattle",0,reinterpret_cast<void *>(on_init_battle_board),
            reinterpret_cast<void **>(&g_init_battle_original)}
    };
    using GetMethodFlags=uint32_t (*)(const void *,uint32_t *);
    auto getFlags=symbol<GetMethodFlags>(g_il2cpp_handle.load(std::memory_order_acquire),
                                         "il2cpp_method_get_flags");
    if(!getFlags) {
        __android_log_print(ANDROID_LOG_ERROR,TAG,
            "Missing il2cpp_method_get_flags: extras disabled to avoid static ABI mixup");
        return false;
    }
    bool any=false;
    for(auto &h:hooks) {
        const void *info=method_find(klass,h.name,h.parameters);
        if(!info)continue;
        uint32_t implementationFlags=0;
        uint32_t flags=getFlags(info,&implementationFlags);
        if((flags&0x0010U)!=0U) {
            __android_log_print(ANDROID_LOG_WARN,TAG,
                "Skipping static redraw method %s (no player instance)",h.name);
            continue;
        }
        const void *ret=get_return(info);
        int retCode=ret?type_code(ret):-1;
        bool safe=(retCode==1); // every interceptor is a void method
        for(int i=0;safe && i<h.parameters;i++) {
            const void *arg=get_param(info,(uint32_t)i);
            int t=arg?type_code(arg):-1;
            // integer/boolean/enum-compatible ABI only, no floating point,
            // object return, or value-type struct arguments.
            safe=t==2 || (t>=3 && t<=11);
        }
        void *entry=nullptr;
        std::memcpy(&entry,info,sizeof(entry));
        if(!safe || !is_executable_il2cpp_address(entry)) {
            __android_log_print(ANDROID_LOG_WARN,TAG,
                "Skipping board redraw %s: unsafe signature or address",h.name);
            continue;
        }
        void *backup=nullptr;
        const int rc=g_hook(entry,h.replacement,&backup);
        if(rc==0 && backup) {
            *h.original=backup;
            any=true;
            __android_log_print(ANDROID_LOG_INFO,TAG,
                "Observing board after player.%s",h.name);
        } else __android_log_print(ANDROID_LOG_ERROR,TAG,
            "player.%s extra hook error=%d backup=%p",h.name,rc,backup);
    }
    return any;
}

// Primary redraw observer delegates into the same read-only capture path.
void observe_draw_all(void *self,int32_t renderOption,const void *method) {
    const DrawAll original=g_original_draw_all;
    if(!original)return;
    original(self,renderOption,method);
    snapshot_board(self);
}

bool install_board_reader(void *handle,void *image,
                          void *(*class_find)(void*,const char*,const char*),
                          const void *(*method_find)(void*,const char*,int),
                          const void *(*get_return)(const void*),
                          const void *(*get_param)(const void*,uint32_t),
                          int (*type_code)(const void*)) {
    void *klass=class_find(image,"","player");
    if(!klass || !g_hook) {board_status(3,250);return false;}
    g_player_class=klass;
    g_find_field=symbol<FindField>(handle,"il2cpp_class_get_field_from_name");
    g_get_field=symbol<GetField>(handle,"il2cpp_field_get_value");
    g_field_type=symbol<FieldType>(handle,"il2cpp_field_get_type");
    g_type_code=symbol<GetType>(handle,"il2cpp_type_get_type");
    g_static_field_get=symbol<StaticFieldGet>(
        handle,"il2cpp_field_static_get_value");
    g_array_len=symbol<ArrayLength>(handle,"il2cpp_array_length");
    g_array_bytes=symbol<ArrayByteLength>(handle,"il2cpp_array_get_byte_length");
    g_array_header=symbol<ArrayHeader>(handle,"il2cpp_array_object_header_size");
    g_object_class=symbol<ObjectClass>(handle,"il2cpp_object_get_class");
    g_array_element_size=symbol<ArrayElementSize>(
        handle,"il2cpp_array_element_size");
    if(!g_find_field || !g_get_field || !g_field_type || !g_type_code ||
       !g_array_len || !g_array_bytes || !g_array_header ||
       !g_object_class || !g_array_element_size){
        board_status(3,251);return false;
    }
    g_board_field=g_find_field(klass,"boardData");
    g_hand_field=g_find_field(klass,"motiData");
    const void *btype=g_board_field?g_field_type(g_board_field):nullptr;
    const int boardType=btype?g_type_code(btype):-1;
    // IL2CPP_TYPE_ARRAY=20 / SZARRAY=29. Reject other layouts.
    if(!g_board_field || (boardType!=20 && boardType!=29)){
        __android_log_print(ANDROID_LOG_ERROR,TAG,
            "Unexpected player.boardData type=%d field=%p",boardType,g_board_field);
        board_status(3,252);return false;
    }
    const char *names[8]={
        "KOMAT_LION","KOMAT_KIRI","KOMAT_ZOU","KOMAT_HIYO",
        "KOMAT_NIWA","KOMAZ_MASK","KOMAZ_SIDE","KOMAZ_REV"
    };
    for(int i=0;i<8;i++) {
        const void *f=g_find_field(klass,names[i]);
        if(f && g_static_field_get) {
            const void *type=g_field_type(f);
            int code=type?g_type_code(type):-1;
            if(code==8 || code==9 || code==4 || code==5 ||
               code==6 || code==7)
                g_static_field_get(f,&g_piece_codes[i]);
        }
    }
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "Board constants types=%d,%d,%d,%d,%d mask=%d side=%d reverse=%d",
        g_piece_codes[0],g_piece_codes[1],g_piece_codes[2],
        g_piece_codes[3],g_piece_codes[4],g_piece_codes[5],
        g_piece_codes[6],g_piece_codes[7]);
    const void *method=method_find(klass,"drawKomaAll",1);
    if(!method || !get_return || !get_param || !type_code){
        board_status(3,253);return false;
    }
    const void *ret=get_return(method);
    const void *param=get_param(method,0);
    int rt=ret?type_code(ret):-1;
    int at=param?type_code(param):-1;
    if(rt!=1 || !(at==2 || (at>=3 && at<=11))){
        __android_log_print(ANDROID_LOG_ERROR,TAG,
            "drawKomaAll return=%d argument=%d; refusing unsafe ABI",rt,at);
        board_status(3,254);return false;
    }
    void *entry=nullptr;
    std::memcpy(&entry,method,sizeof(entry));
    if(!is_executable_il2cpp_address(entry)){
        board_status(3,255);return false;
    }
    void *backup=nullptr;
    int result=g_hook(entry,reinterpret_cast<void*>(observe_draw_all),&backup);
    if(result!=0 || !backup){
        __android_log_print(ANDROID_LOG_ERROR,TAG,
            "drawKomaAll inline hook result=%d backup=%p",result,backup);
        board_status(3,256);return false;
    }
    g_original_draw_all=reinterpret_cast<DrawAll>(backup);
    init_verified_turn_reader(handle,image,klass,method_find,
        reinterpret_cast<void *(*)(void *,const char *,const char *)>(class_find));
    // Exclude JNI/render frame polling: read the live board immediately
    // after its real game render, move, refresh and turn-label methods.
    // These callbacks are enabled only if every parameter has a safe ABI.
    install_extra_board_redraw_hooks(image,klass,method_find,
        get_return,get_param,type_code);
    board_status(2,1);
    __android_log_print(ANDROID_LOG_INFO,TAG,
        "3x4 board reader installed: player.drawKomaAll @%p",entry);
    return true;
}

bool install_hooks(void *handle) {
    using DomainGet = void *(*)();
    using Attach = void *(*)(void *);
    using Detach = void (*)(void *);
    using AssemblyOpen = void *(*)(void *, const char *);
    using ImageGet = void *(*)(void *);
    using ClassFind = void *(*)(void *, const char *, const char *);
    using MethodFind = const void *(*)(void *, const char *, int);
    using MethodGetReturnType = const void *(*)(const void *);
    using MethodGetParamType = const void *(*)(const void *, uint32_t);
    using TypeGetType = int (*)(const void *);
    auto method_get_return = symbol<MethodGetReturnType>(handle, "il2cpp_method_get_return_type");
    auto method_get_param = symbol<MethodGetParamType>(handle, "il2cpp_method_get_param");
    auto type_get_type = symbol<TypeGetType>(handle, "il2cpp_type_get_type");
    auto domain_get = symbol<DomainGet>(handle, "il2cpp_domain_get");
    auto attach = symbol<Attach>(handle, "il2cpp_thread_attach");
    auto detach = symbol<Detach>(handle, "il2cpp_thread_detach");
    auto assembly_open = symbol<AssemblyOpen>(handle, "il2cpp_domain_assembly_open");
    auto image_get = symbol<ImageGet>(handle, "il2cpp_assembly_get_image");
    auto class_find = symbol<ClassFind>(handle, "il2cpp_class_from_name");
    auto method_find = symbol<MethodFind>(handle, "il2cpp_class_get_method_from_name");
    if (!domain_get || !attach || !assembly_open || !image_get ||
            !class_find || !method_find || !g_hook) {
        stage(110, "Missing IL2CPP export or LSPosed native hook function");
        return false;
    }
    void *domain = domain_get();
    if (!domain) {
        stage(120, "IL2CPP domain is not initialized");
        return false;
    }
    void *thread = attach(domain);
    if (!thread) {
        stage(130, "il2cpp_thread_attach failed");
        return false;
    }

    void *assembly = assembly_open(domain, "Assembly-CSharp");
    if (!assembly) assembly = assembly_open(domain, "Assembly-CSharp.dll");
    void *image = assembly ? image_get(assembly) : nullptr;
    if (!image) {
        stage(140, "Assembly-CSharp image not loaded");
        if (detach) detach(thread);
        return false;
    }

    // Only the ABI-verified board reader installs active IL2CPP hooks.
    const bool boardOK=install_board_reader(handle,image,class_find,
        method_find,method_get_return,method_get_param,type_get_type);
    if(detach)detach(thread);
    if(boardOK){
        g_hook_installed.store(true,std::memory_order_release);
        status(2,1);
        return true;
    }
    return false;

}

// Load notifications are NOT equivalent to an initialized Unity VM.
// The supplied tombstone shows il2cpp_domain_get triggering a dereference
// at x0+0x135, with x0=0, when called during Unity startup.
void init_finished(int result, const char *name) {
    if (!result) {
        __android_log_print(ANDROID_LOG_ERROR, TAG, "%s returned failure", name);
        status(3, 213);
        return;
    }
    g_runtime_ready.store(true, std::memory_order_release);
    __android_log_print(ANDROID_LOG_INFO, TAG, "%s completed; metadata allowed", name);
    stage(220, "IL2CPP initialization completed");
    start_worker(g_il2cpp_handle.load(std::memory_order_acquire));
}
int intercept_init(const char *name) {
    const auto original = g_init_original;
    if (!original) { status(3, 212); return 0; }
    int result = original(name);
    init_finished(result, "il2cpp_init");
    return result;
}
int intercept_init_utf16(const uint16_t *name) {
    const auto original = g_init_utf16_original;
    if (!original) { status(3, 212); return 0; }
    int result = original(name);
    init_finished(result, "il2cpp_init_utf16");
    return result;
}
void install_init_hooks(void *handle) {
    if (!handle || !g_hook || g_init_hooks_attempted.exchange(true)) return;
    bool any = false;
    void *init = dlsym(handle, "il2cpp_init");
    void *init16 = dlsym(handle, "il2cpp_init_utf16");
    if (init) {
        void *backup = nullptr;
        int result = g_hook(init, reinterpret_cast<void *>(intercept_init), &backup);
        if (result == 0 && backup) {
            g_init_original = reinterpret_cast<Il2cppInit>(backup);
            any = true;
            __android_log_print(ANDROID_LOG_INFO, TAG, "Hooked il2cpp_init");
        } else __android_log_print(ANDROID_LOG_ERROR, TAG,
            "il2cpp_init hook failure: code=%d backup=%p", result, backup);
    }
    if (init16) {
        void *backup = nullptr;
        int result = g_hook(init16, reinterpret_cast<void *>(intercept_init_utf16), &backup);
        if (result == 0 && backup) {
            g_init_utf16_original = reinterpret_cast<Il2cppInitUtf16>(backup);
            any = true;
            __android_log_print(ANDROID_LOG_INFO, TAG, "Hooked il2cpp_init_utf16");
        } else __android_log_print(ANDROID_LOG_ERROR, TAG,
            "il2cpp_init_utf16 hook failure: code=%d backup=%p", result, backup);
    }
    if (any) stage(210, "Waiting for il2cpp_init to finish");
    else {
        stage(211, "Cannot hook IL2CPP initialization entry points");
        status(3, 211);
    }
}

void *worker(void *opaque) {
    void *handle = opaque;
    // On first loader notification IL2CPP may not yet have an initialized
    // domain/assembly; wait asynchronously instead of blocking dlopen.
    for (int i = 0; i < 180 && !g_hook_installed.load(); ++i) {
        if (install_hooks(handle)) break;
        usleep(500000);
    }
    if (!g_hook_installed.load()) {
        int detail = g_stage.load(std::memory_order_relaxed);
        __android_log_print(ANDROID_LOG_ERROR, TAG,
            "Failed to hook after 90s: phase=%d", detail);
        status(3, detail);
    }
    return nullptr;
}

void start_worker(void *handle) {
    if (!handle || !g_hook) return;
    g_il2cpp_handle.store(handle, std::memory_order_release);
    if (!g_runtime_ready.load(std::memory_order_acquire)) {
        install_init_hooks(handle);
        return;
    }
    if (g_worker_started.exchange(true)) return;
    pthread_t th;
    int rc = pthread_create(&th, nullptr, worker, handle);
    if (!rc) pthread_detach(th);
    else {
        g_worker_started.store(false);
        __android_log_print(ANDROID_LOG_ERROR, TAG, "pthread_create failed: %d", rc);
        status(3, rc);
    }
}

void *poll_for_il2cpp(void *) {
    for (int attempt = 0; attempt < 180 && !g_worker_started.load(); ++attempt) {
        void *handle = dlopen("libil2cpp.so", RTLD_NOW | RTLD_NOLOAD);
        if (handle) {
            __android_log_print(ANDROID_LOG_INFO, TAG,
                "libil2cpp.so rediscovered with RTLD_NOLOAD");
            start_worker(handle);
            break;
        }
        usleep(500000);
    }
    if (!g_init_hooks_attempted.load() && !g_worker_started.load() &&
            !g_hook_installed.load()) {
        stage(100, "Never received libil2cpp.so load notification");
        status(3, 100);
    }
    return nullptr;
}

void start_poller() {
    if (g_poller_started.exchange(true)) return;
    pthread_t th;
    int result = pthread_create(&th, nullptr, poll_for_il2cpp, nullptr);
    if (result == 0) pthread_detach(th);
    else status(3, 101);
}

void on_library_loaded(const char *name, void *handle) {
    if (name && std::strstr(name, "libil2cpp.so")) {
        stage(105, "libil2cpp.so load notification observed");
        start_worker(handle);
    }
}
} // namespace

extern "C" JNIEXPORT void JNICALL
Java_com_atuy_dobutsueval_lsposed_MiniBoardOverlay_nativeSetTablebasePath(
        JNIEnv *env,jclass,jstring path) {
    if(!path)return;
    const char *value=env->GetStringUTFChars(path,nullptr);
    if(!value)return;
    {
        std::lock_guard<std::mutex> lock(g_table_mutex);
        g_table_path=value;
    }
    env->ReleaseStringUTFChars(path,value);
}

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *) {
    JNIEnv *env = nullptr;
    if (vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6) != JNI_OK)
        return JNI_ERR;
    jclass cls = env->FindClass("com/atuy/dobutsueval/lsposed/GameOverlayController");
    if (!cls) {
        __android_log_print(ANDROID_LOG_ERROR, TAG, "JNI class lookup failed");
        return JNI_ERR;
    }
    g_java_class = reinterpret_cast<jclass>(env->NewGlobalRef(cls));
    env->DeleteLocalRef(cls);
    if (!g_java_class) return JNI_ERR;
    g_score_callback = env->GetStaticMethodID(g_java_class, "onNativeScore", "(II)V");
    g_status_callback = env->GetStaticMethodID(g_java_class, "onNativeStatus", "(II)V");
    g_activity_callback = env->GetStaticMethodID(g_java_class, "onNativeActivity", "(II)V");
    g_new_match_callback = env->GetStaticMethodID(g_java_class, "onNativeNewMatch", "(I)V");
    g_committed_callback = env->GetStaticMethodID(g_java_class, "onNativeCommittedScore", "(III)V");
    if (!g_score_callback || !g_status_callback || !g_activity_callback ||
            !g_new_match_callback || !g_committed_callback)
        return JNI_ERR;
    jclass board=env->FindClass(
        "com/atuy/dobutsueval/lsposed/MiniBoardOverlay");
    if(!board)return JNI_ERR;
    g_board_java_class=reinterpret_cast<jclass>(env->NewGlobalRef(board));
    env->DeleteLocalRef(board);
    if(!g_board_java_class)return JNI_ERR;
    g_board_callback=env->GetStaticMethodID(g_board_java_class,
        "onNativeBoard","([I[I[IIIII)V");
    g_board_status_callback=env->GetStaticMethodID(g_board_java_class,
        "onNativeBoardStatus","(II)V");
    g_board_reset_callback=env->GetStaticMethodID(g_board_java_class,
        "onNewGame","()V");
    if(!g_board_callback || !g_board_status_callback ||
       !g_board_reset_callback)return JNI_ERR;
    g_vm.store(vm, std::memory_order_release);
    status(1, g_stage.load());
    if (g_hook_installed.load()) status(2, NCAND); // latest native result is in logcat
    return JNI_VERSION_1_6;
}

extern "C" __attribute__((visibility("default"), used))
LoadedCallback native_init(const NativeAPIEntries *entries) {
    if (!entries || !entries->hook_func) {
        __android_log_print(ANDROID_LOG_ERROR, TAG, "LSPosed native hook API unavailable");
        status(3, -2);
        return nullptr;
    }
    g_hook = entries->hook_func;
    __android_log_print(ANDROID_LOG_INFO, TAG,
        "Native hook entry attached (API %u)", entries->version);
    void *existing = dlopen("libil2cpp.so", RTLD_NOW | RTLD_NOLOAD);
    if (existing) {
        stage(105, "libil2cpp.so already loaded");
        start_worker(existing);
    } else {
        stage(100, "Waiting for libil2cpp.so");
        start_poller(); // Covers missed dlopen notifications.
    }
    return on_library_loaded;
}
