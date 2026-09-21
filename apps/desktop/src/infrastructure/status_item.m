#import <AppKit/AppKit.h>

// macOS menu bar status item. GPUI owns the NSApplication run loop but exposes
// no status item API, so this file owns the AppKit objects and forwards clicks
// back to Rust through a C callback. Every label is a Rust-owned UTF-8 string;
// nothing user visible is written here.
//
// Menu item tags are the command ids shared with status_item.rs.

enum {
    NexusStatusItemToggleWindow = 0,
    NexusStatusItemNewTask = 1,
    NexusStatusItemQuit = 2,
};

typedef void (*NexusStatusItemCommand)(int32_t command);

@interface NexusStatusItemTarget : NSObject
- (void)performCommand:(NSMenuItem *)sender;
@end

static NexusStatusItemCommand gCommand = NULL;
static NSStatusItem *gStatusItem = nil;
static NSMenuItem *gToggleWindowItem = nil;
static NSMenuItem *gNewTaskItem = nil;
static NSMenuItem *gQuitItem = nil;
static NexusStatusItemTarget *gTarget = nil;

@implementation NexusStatusItemTarget
- (void)performCommand:(NSMenuItem *)sender {
    // AppKit dispatches menu actions on the main thread, and the status item
    // keeps this callback valid for the whole process lifetime.
    NexusStatusItemCommand command = gCommand;
    if (command != NULL) command((int32_t)sender.tag);
}
@end

static NSImage *NexusStatusItemImage(const uint8_t *bytes, size_t length) {
    if (bytes == NULL || length == 0) return nil;
    NSData *data = [NSData dataWithBytes:bytes length:length];
    NSImage *image = [[NSImage alloc] initWithData:data];
    if (image == nil) return nil;
    // Status bar images are tinted by AppKit from their alpha channel.
    image.template = YES;
    image.size = NSMakeSize(18.0, 18.0);
    return image;
}

static NSMenuItem *NexusStatusItemMenuItem(const char *title, NSInteger tag) {
    NSString *label = title ? @(title) : @"";
    NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:label
                                                 action:@selector(performCommand:)
                                          keyEquivalent:@""];
    item.target = gTarget;
    item.tag = tag;
    return item;
}

static void NexusStatusItemSetTitle(NSMenuItem *item, const char *title) {
    if (item != nil) item.title = title ? @(title) : @"";
}

void nexus_status_item_install(NexusStatusItemCommand command,
                               const char *toggle_window_title,
                               const char *new_task_title,
                               const char *quit_title,
                               const uint8_t *icon_bytes,
                               size_t icon_length) {
    if (gStatusItem != nil) return;
    gCommand = command;
    gTarget = [[NexusStatusItemTarget alloc] init];

    gToggleWindowItem = NexusStatusItemMenuItem(toggle_window_title, NexusStatusItemToggleWindow);
    gNewTaskItem = NexusStatusItemMenuItem(new_task_title, NexusStatusItemNewTask);
    gQuitItem = NexusStatusItemMenuItem(quit_title, NexusStatusItemQuit);

    NSMenu *menu = [[NSMenu alloc] init];
    // Status items are outside the responder chain, so automatic item
    // validation would disable everything while the app is hidden.
    menu.autoenablesItems = NO;
    [menu addItem:gToggleWindowItem];
    [menu addItem:gNewTaskItem];
    [menu addItem:[NSMenuItem separatorItem]];
    [menu addItem:gQuitItem];

    gStatusItem = [[NSStatusBar systemStatusBar] statusItemWithLength:NSSquareStatusItemLength];
    gStatusItem.button.image = NexusStatusItemImage(icon_bytes, icon_length);
    gStatusItem.button.imagePosition = NSImageOnly;
    gStatusItem.menu = menu;
    gStatusItem.visible = YES;
}

void nexus_status_item_set_titles(const char *toggle_window_title,
                                  const char *new_task_title,
                                  const char *quit_title) {
    NexusStatusItemSetTitle(gToggleWindowItem, toggle_window_title);
    NexusStatusItemSetTitle(gNewTaskItem, new_task_title);
    NexusStatusItemSetTitle(gQuitItem, quit_title);
}

// 1 when the application is hidden, so Rust can label the toggle and decide
// what the toggle should do.
int32_t nexus_status_item_application_hidden(void) {
    return NSApplication.sharedApplication.hidden ? 1 : 0;
}

// Reactivating a hidden application does not always order its windows back on
// screen, so unhide before GPUI activates it.
void nexus_status_item_unhide_application(void) {
    [NSApplication.sharedApplication unhide:nil];
}
