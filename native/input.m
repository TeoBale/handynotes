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

static void installSwipeMonitor(void) {
    static id monitor;
    if (monitor != nil) {
        return;
    }

    __block BOOL tracking = NO;
    __block BOOL handled = NO;
    __block CGFloat horizontal = 0;
    __block CGFloat verticalTravel = 0;

    monitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskScrollWheel
        handler:^NSEvent *(NSEvent *event) {
            if (!event.hasPreciseScrollingDeltas || event.momentumPhase != NSEventPhaseNone) {
                return event;
            }

            NSEventPhase phase = event.phase;
            if (phase & (NSEventPhaseEnded | NSEventPhaseCancelled)) {
                tracking = NO;
                return event;
            }
            if (phase & NSEventPhaseBegan) {
                tracking = event.window != nil && event.window == NSApp.keyWindow;
                handled = NO;
                horizontal = 0;
                verticalTravel = 0;
            }

            if (!tracking || event.window == nil || event.window != NSApp.keyWindow) {
                tracking = NO;
                return event;
            }
            if (handled) {
                return event;
            }

            if (phase & (NSEventPhaseBegan | NSEventPhaseChanged)) {
                // Normalize to the physical direction regardless of the scroll preference.
                CGFloat delta = event.scrollingDeltaX;
                horizontal += event.isDirectionInvertedFromDevice ? -delta : delta;
                verticalTravel += fabs(event.scrollingDeltaY);
                if (fabs(horizontal) >= 38 && fabs(horizontal) > 1.6 * verticalTravel) {
                    handled = YES;
                    deliverEvent(horizontal > 0 ? InputSwipeLeft : InputSwipeRight);
                }
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
