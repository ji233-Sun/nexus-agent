#import <AVFoundation/AVFoundation.h>
#import <Speech/Speech.h>

@interface NexusSpeechSession : NSObject
@property(nonatomic, strong) AVAudioEngine *engine;
@property(nonatomic, strong) SFSpeechRecognizer *recognizer;
@property(nonatomic, strong) SFSpeechAudioBufferRecognitionRequest *request;
@property(nonatomic, strong) SFSpeechRecognitionTask *task;
@property(nonatomic, copy) NSString *result;
@property(nonatomic, copy) NSString *error;
@property(nonatomic) BOOL done;
@property(nonatomic) BOOL stopped;
@property(nonatomic) BOOL tapInstalled;
- (void)stopAndCancel:(BOOL)cancel;
@end

@implementation NexusSpeechSession
- (void)stopAndCancel:(BOOL)cancel {
    BOOL stopAudio = NO;
    BOOL removeTap = NO;
    @synchronized (self) {
        if (!_stopped) {
            stopAudio = YES;
            removeTap = _tapInstalled;
            _tapInstalled = NO;
            _stopped = YES;
        }
        if (cancel) _done = YES;
    }
    // Never hold the callback lock while stopping the engine: stop may wait for a tap.
    if (stopAudio) {
        [_engine stop];
        if (removeTap) [_engine.inputNode removeTapOnBus:0];
        [_request endAudio];
    }
    if (cancel) [_task cancel];
}
@end

static char *NexusCopyString(NSString *value) {
    if (!value) return NULL;
    return strdup(value.UTF8String ?: "");
}

void nexus_speech_free_string(char *value) { free(value); }

// Values are normalized for Rust: 0 unknown, 1 not determined, 2 denied/restricted, 3 authorized.
int nexus_microphone_authorization(void) {
    switch ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]) {
        case AVAuthorizationStatusNotDetermined: return 1;
        case AVAuthorizationStatusAuthorized: return 3;
        default: return 2;
    }
}

void nexus_request_microphone_authorization(void) {
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(__unused BOOL allowed) {}];
}

int nexus_speech_authorization(void) {
    switch (SFSpeechRecognizer.authorizationStatus) {
        case SFSpeechRecognizerAuthorizationStatusNotDetermined: return 1;
        case SFSpeechRecognizerAuthorizationStatusAuthorized: return 3;
        default: return 2;
    }
}

void nexus_request_speech_authorization(void) {
    [SFSpeechRecognizer requestAuthorization:^(__unused SFSpeechRecognizerAuthorizationStatus status) {}];
}

void *nexus_speech_start(const char *locale_name, char **error_out) {
    @autoreleasepool {
    NexusSpeechSession *session = [NexusSpeechSession new];
    NSLocale *locale = locale_name ? [[NSLocale alloc] initWithLocaleIdentifier:[NSString stringWithUTF8String:locale_name]] : NSLocale.currentLocale;
    session.recognizer = [[SFSpeechRecognizer alloc] initWithLocale:locale];
    if (!session.recognizer || !session.recognizer.available) {
        *error_out = NexusCopyString(@"Speech recognition is unavailable for the selected locale");
        return NULL;
    }
    session.engine = [AVAudioEngine new];
    session.request = [SFSpeechAudioBufferRecognitionRequest new];
    session.request.shouldReportPartialResults = NO;
    AVAudioInputNode *input = session.engine.inputNode;
    AVAudioFormat *format = [input outputFormatForBus:0];
    if (format.channelCount == 0) {
        *error_out = NexusCopyString(@"audio input has no channels");
        return NULL;
    }
    if (format.sampleRate <= 0) {
        *error_out = NexusCopyString(@"audio input has an invalid sample rate");
        return NULL;
    }
    __weak NexusSpeechSession *weakSession = session;
    [input installTapOnBus:0 bufferSize:1024 format:format block:^(AVAudioPCMBuffer *buffer, __unused AVAudioTime *when) {
        NexusSpeechSession *strongSession = weakSession;
        if (!strongSession) return;
        @synchronized (strongSession) {
            if (!strongSession.stopped) [strongSession.request appendAudioPCMBuffer:buffer];
        }
    }];
    session.tapInstalled = YES;
    session.task = [session.recognizer recognitionTaskWithRequest:session.request resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
        NexusSpeechSession *strongSession = weakSession;
        if (!strongSession) return;
        @synchronized (strongSession) {
            if (result) strongSession.result = result.bestTranscription.formattedString;
            if (error) strongSession.error = error.localizedDescription;
            if (error || result.isFinal) strongSession.done = YES;
        }
    }];
    NSError *start_error = nil;
    [session.engine prepare];
    if (![session.engine startAndReturnError:&start_error]) {
        [session stopAndCancel:YES];
        *error_out = NexusCopyString(start_error.localizedDescription ?: @"failed to start audio input");
        return NULL;
    }
    return (__bridge_retained void *)session;
    }
}

void nexus_speech_stop(void *opaque) {
    @autoreleasepool {
        if (opaque) [(__bridge NexusSpeechSession *)opaque stopAndCancel:NO];
    }
}

void nexus_speech_cancel(void *opaque) {
    @autoreleasepool {
        if (opaque) [(__bridge NexusSpeechSession *)opaque stopAndCancel:YES];
    }
}

char *nexus_speech_take_result(void *opaque, bool *done) {
    @autoreleasepool {
    NexusSpeechSession *session = (__bridge NexusSpeechSession *)opaque;
    @synchronized (session) {
        *done = session.done;
        if (!session.done) return NULL;
        return NexusCopyString(session.error ? [@"ERROR: " stringByAppendingString:session.error] : (session.result ?: @""));
    }
    }
}

void nexus_speech_free_session(void *opaque) {
    @autoreleasepool {
    if (!opaque) return;
    NexusSpeechSession *session = (__bridge_transfer NexusSpeechSession *)opaque;
    [session stopAndCancel:YES];
    }
}

char *nexus_speech_status(const char *locale_name) {
    NSLocale *locale = locale_name ? [[NSLocale alloc] initWithLocaleIdentifier:[NSString stringWithUTF8String:locale_name]] : NSLocale.currentLocale;
    SFSpeechRecognizer *recognizer = [[SFSpeechRecognizer alloc] initWithLocale:locale];
    NSString *mic = @[@"not-determined", @"restricted", @"denied", @"authorized"][[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]];
    SFSpeechRecognizerAuthorizationStatus speech = SFSpeechRecognizer.authorizationStatus;
    NSArray *speech_names = @[@"not-determined", @"denied", @"restricted", @"authorized"];
    NSString *speech_name = speech <= SFSpeechRecognizerAuthorizationStatusAuthorized ? speech_names[speech] : @"unknown";
    NSString *on_device = @"unsupported";
    if (@available(macOS 10.15, *)) on_device = recognizer.supportsOnDeviceRecognition ? @"supported" : @"unsupported";
    return NexusCopyString([NSString stringWithFormat:@"microphone=%@; speech=%@; locale=%@; service=%@; supportsOnDeviceRecognition=%@", mic, speech_name, locale.localeIdentifier, recognizer.available ? @"available" : @"unavailable", on_device]);
}

char *nexus_speech_locales(void) {
    NSArray *names = [[SFSpeechRecognizer supportedLocales].allObjects valueForKey:@"localeIdentifier"];
    return NexusCopyString([[names sortedArrayUsingSelector:@selector(localizedCaseInsensitiveCompare:)] componentsJoinedByString:@"\n"]);
}
