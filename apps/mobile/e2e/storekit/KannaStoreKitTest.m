// Included ONLY by the opt-in dev simulator config plugin. Never a receipt verifier.
#import <TargetConditionals.h>
#if DEBUG && TARGET_OS_SIMULATOR
#import <React/RCTBridgeModule.h>
#import <StoreKitTest/StoreKitTest.h>
@interface KannaStoreKitTest : NSObject <RCTBridgeModule>
@property(nonatomic, strong) SKTestSession *session;
@end
@implementation KannaStoreKitTest
RCT_EXPORT_MODULE();
+ (BOOL)requiresMainQueueSetup { return YES; }
- (NSDictionary *)constantsToExport { return @{ @"restoreOnly": @([[NSProcessInfo processInfo].arguments containsObject:@"--storekit-restore"]) }; }
- (dispatch_queue_t)methodQueue { return dispatch_get_main_queue(); }
RCT_REMAP_METHOD(start, startWithResolver:(RCTPromiseResolveBlock)resolve rejecter:(RCTPromiseRejectBlock)reject) {
  NSError *error = nil;
  self.session = [[SKTestSession alloc] initWithConfigurationFileNamed:@"KannaCloud" error:&error];
  if (!self.session) { reject(@"storekit_test", @"Could not load isolated StoreKit configuration", error); return; }
  [self.session resetToDefaultState];
  if (![[NSProcessInfo processInfo].arguments containsObject:@"--storekit-restore"]) [self.session clearTransactions];
  self.session.disableDialogs = YES;
  resolve(@YES);
}
RCT_REMAP_METHOD(setPending, pending:(BOOL)pending resolver:(RCTPromiseResolveBlock)resolve rejecter:(RCTPromiseRejectBlock)reject) {
  self.session.askToBuyEnabled = pending;
  resolve(@YES);
}
RCT_REMAP_METHOD(approve, approveWithResolver:(RCTPromiseResolveBlock)resolve rejecter:(RCTPromiseRejectBlock)reject) {
  for (SKTestTransaction *transaction in self.session.allTransactions) {
    if (transaction.state == SKPaymentTransactionStateDeferred) {
      NSError *error = nil;
      if (![self.session approveAskToBuyTransactionWithIdentifier:transaction.identifier error:&error]) {
        reject(@"storekit_test", @"Could not approve deferred fixture", error); return;
      }
    }
  }
  resolve(@YES);
}
RCT_REMAP_METHOD(clear, clearWithResolver:(RCTPromiseResolveBlock)resolve rejecter:(RCTPromiseRejectBlock)reject) {
  [self.session clearTransactions]; resolve(@YES);
}
@end
#endif
