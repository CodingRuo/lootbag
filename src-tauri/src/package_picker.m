#import <AppKit/AppKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include <stdlib.h>
#include <string.h>

// Called synchronously on Tauri's main thread. The caller releases the copied path.
char *select_import_path(void *parent) {
    @autoreleasepool {
        NSOpenPanel *panel = [NSOpenPanel openPanel];
        panel.message = @"Ordner oder ZIP-Paket zum Importieren auswählen";
        panel.prompt = @"Auswählen";
        panel.allowedContentTypes = @[UTTypeFolder, UTTypeZIP];
        panel.allowsOtherFileTypes = NO;
        // Set these after the content types: macOS 27 also derives them from types.
        panel.canChooseFiles = YES;
        panel.canChooseDirectories = YES;
        panel.allowsMultipleSelection = NO;
        [panel beginSheetModalForWindow:(__bridge NSWindow *)parent
                     completionHandler:^(NSModalResponse response) { (void)response; }];
        if ([panel runModal] != NSModalResponseOK) {
            return NULL;
        }
        return strdup(panel.URL.fileSystemRepresentation);
    }
}

void release_import_path(char *path) {
    free(path);
}
