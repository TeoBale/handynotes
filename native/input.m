#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <math.h>
#import <pthread.h>
#import <stdbool.h>
#import <stdint.h>

enum {
    InputToggle = 1,
    InputSwipeLeft = 2,
    InputSwipeRight = 3,
};

static void (*deliverEvent)(int32_t);

static const EventHotKeyID toggleID = {.signature = 0x484E5445, .id = 1}; // HNTE
static BOOL hotKeyHeld;

static OSStatus handleHotKey(EventHandlerCallRef next, EventRef event, void *context) {
    (void)next;
    (void)context;

    EventHotKeyID key;
    OSStatus result = GetEventParameter(event, kEventParamDirectObject, typeEventHotKeyID,
                                        NULL, sizeof(key), NULL, &key);
    if (result != noErr || key.signature != toggleID.signature || key.id != toggleID.id) {
        return eventNotHandledErr;
    }

    if (GetEventKind(event) == kEventHotKeyReleased) {
        hotKeyHeld = NO;
    } else if (!hotKeyHeld) {
        hotKeyHeld = YES;
        deliverEvent(InputToggle);
    }
    return noErr;
}

typedef NS_ENUM(NSUInteger, ScrollAxis) {
    ScrollAxisPending,
    ScrollAxisVertical,
    ScrollAxisHorizontal,
};

typedef struct {
    ScrollAxis axis;
    CGFloat horizontal;
    CGFloat verticalTravel;
    BOOL turnedPage;
} SwipeGesture;

static int32_t trackSwipe(SwipeGesture *gesture, CGFloat x, CGFloat y) {
    if (gesture->axis == ScrollAxisVertical || gesture->turnedPage) {
        return 0;
    }

    gesture->horizontal += x;
    if (gesture->axis == ScrollAxisPending) {
        gesture->verticalTravel += fabs(y);
        CGFloat horizontal = fabs(gesture->horizontal);
        if (fmax(horizontal, gesture->verticalTravel) < 6) {
            return 0;
        }
        gesture->axis = horizontal > 1.6 * gesture->verticalTravel
            ? ScrollAxisHorizontal : ScrollAxisVertical;
    }

    if (gesture->axis == ScrollAxisHorizontal && fabs(gesture->horizontal) >= 38) {
        gesture->turnedPage = YES;
        return gesture->horizontal > 0 ? InputSwipeLeft : InputSwipeRight;
    }
    return 0;
}

static void installSwipeMonitor(void) {
    static id monitor;
    if (monitor != nil) {
        return;
    }

    __block BOOL tracking = NO;
    __block BOOL replaying = NO;
    __block __weak NSWindow *gestureWindow;
    __block SwipeGesture gesture = {0};
    NSMutableArray<NSEvent *> *pending = [NSMutableArray array];

    monitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskScrollWheel
        handler:^NSEvent *(NSEvent *event) {
            if (replaying || !event.hasPreciseScrollingDeltas) {
                return event;
            }

            NSWindow *window = event.window;
            NSEventPhase momentum = event.momentumPhase;
            if (momentum != NSEventPhaseNone) {
                // The axis stays locked through momentum, even after a page turn.
                NSWindow *target = window ?: NSApp.keyWindow;
                if (target != nil && target == gestureWindow) {
                    if (momentum & (NSEventPhaseEnded | NSEventPhaseCancelled)) {
                        gestureWindow = nil;
                    }
                    return gesture.axis == ScrollAxisVertical ? event : nil;
                }
                return event;
            }

            NSEventPhase phase = event.phase;
            if (phase & NSEventPhaseBegan) {
                tracking = window != nil && window == NSApp.keyWindow;
                gestureWindow = tracking ? window : nil;
                gesture = (SwipeGesture){0};
                [pending removeAllObjects];
            }

            if (!tracking || window != gestureWindow || window != NSApp.keyWindow) {
                tracking = NO;
                [pending removeAllObjects];
                return event;
            }
            if (phase & (NSEventPhaseEnded | NSEventPhaseCancelled)) {
                tracking = NO;
                [pending removeAllObjects];
                return gesture.axis == ScrollAxisVertical ? event : nil;
            }

            // Normalize to the physical direction regardless of the scroll preference.
            CGFloat x = event.scrollingDeltaX;
            CGFloat y = event.scrollingDeltaY;
            int32_t pageTurn = trackSwipe(&gesture, event.isDirectionInvertedFromDevice ? -x : x, y);
            if (gesture.axis == ScrollAxisHorizontal) {
                [pending removeAllObjects];
                if (pageTurn != 0) {
                    deliverEvent(pageTurn);
                }
                return nil;
            }
            if (gesture.axis == ScrollAxisPending) {
                // Hold the initial deltas until the intent is clear, so even a
                // swipe that starts with vertical drift cannot move the editor.
                if ((phase & NSEventPhaseBegan) || x != 0 || y != 0) {
                    [pending addObject:event];
                }
                return nil;
            }
            if (pending.count != 0) {
                // Preserve the first deltas and gesture phases for vertical scrolling.
                replaying = YES;
                for (NSEvent *buffered in pending) {
                    [NSApp sendEvent:buffered];
                }
                replaying = NO;
                [pending removeAllObjects];
            }
            return event;
        }];
}

int32_t handynotes_input_install(void (*deliver)(int32_t)) {
    static EventHotKeyRef hotKey;
    static EventHandlerRef handler;
    if (!pthread_main_np()) {
        return paramErr;
    }
    deliverEvent = deliver;
    installSwipeMonitor();

    if (hotKey != NULL) {
        return noErr;
    }

    EventTargetRef target = GetApplicationEventTarget();
    if (handler == NULL) {
        const EventTypeSpec types[] = {
            {kEventClassKeyboard, kEventHotKeyPressed},
            {kEventClassKeyboard, kEventHotKeyReleased},
        };
        OSStatus status = InstallEventHandler(target, handleHotKey, 2, types, NULL, &handler);
        if (status != noErr) {
            return status;
        }
    }

    return RegisterEventHotKey(kVK_ANSI_N, optionKey, toggleID, target,
                               kEventHotKeyExclusive, &hotKey);
}

// Tracks the top row even when AppKit's traffic lights own the mouse event.
@interface HNToolbarRegion : NSView
@end

@implementation HNToolbarRegion
- (NSView *)hitTest:(NSPoint)point { return nil; }

- (void)setControlsVisible:(BOOL)visible {
    for (NSUInteger type = NSWindowCloseButton; type <= NSWindowZoomButton; type++) {
        NSButton *button = [self.window standardWindowButton:type];
        if (visible && button.hidden) {
            button.alphaValue = 0;
            button.hidden = NO;
            [NSAnimationContext runAnimationGroup:^(NSAnimationContext *context) {
                context.duration = NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceMotion ? 0 : 0.11;
                button.animator.alphaValue = 1;
            } completionHandler:nil];
        } else if (!visible) {
            button.hidden = YES;
        }
    }
    deliverEvent(visible ? 4 : 5);
}
- (void)mouseEntered:(NSEvent *)event { [self setControlsVisible:YES]; }
- (void)mouseExited:(NSEvent *)event { [self setControlsVisible:NO]; }
@end

bool handynotes_window_is_on_active_space(void *ptr) {
    NSView *content = (__bridge NSView *)ptr;
    return content.window.isOnActiveSpace;
}

void handynotes_configure_window(void *ptr) {
    NSView *content = (__bridge NSView *)ptr;
    NSWindow *window = content.window;
    window.collectionBehavior |= NSWindowCollectionBehaviorMoveToActiveSpace;

    const CGFloat height = 40;
    NSRect frame = content.bounds;
    frame.origin.y = content.isFlipped ? NSMinY(frame) : NSMaxY(frame) - height;
    frame.size.height = height;
    HNToolbarRegion *region = [[HNToolbarRegion alloc] initWithFrame:frame];
    region.autoresizingMask = NSViewWidthSizable |
        (content.isFlipped ? NSViewMaxYMargin : NSViewMinYMargin);
    [content addSubview:region];
    [region addTrackingArea:[[NSTrackingArea alloc] initWithRect:NSZeroRect
        options:NSTrackingInVisibleRect | NSTrackingActiveAlways | NSTrackingMouseEnteredAndExited
        owner:region userInfo:nil]];
    NSPoint mouse = [region convertPoint:window.mouseLocationOutsideOfEventStream fromView:nil];
    [region setControlsVisible:NSPointInRect(mouse, region.bounds)];
}
