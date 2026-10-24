"""Model-only generic-password storage; secrets never enter process arguments."""
import ctypes as C

SERVICE = b"kanna-model-agent-sign-in"


def entry(account, value=None):
    security = C.CDLL('/System/Library/Frameworks/Security.framework/Security')
    core = C.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
    ptr = C.c_void_p
    security.SecKeychainFindGenericPassword.argtypes = [ptr, C.c_uint32, ptr,
        C.c_uint32, ptr, C.POINTER(C.c_uint32), C.POINTER(ptr), C.POINTER(ptr)]
    security.SecKeychainAddGenericPassword.argtypes = [ptr, C.c_uint32, ptr,
        C.c_uint32, ptr, C.c_uint32, ptr, C.POINTER(ptr)]
    security.SecKeychainItemModifyAttributesAndData.argtypes = [ptr, ptr, C.c_uint32, ptr]
    security.SecKeychainItemFreeContent.argtypes = [ptr, ptr]
    core.CFRelease.argtypes = [ptr]
    account = account.encode()
    length, data, item = C.c_uint32(), ptr(), ptr()
    status = security.SecKeychainFindGenericPassword(None, len(SERVICE), SERVICE,
        len(account), account, C.byref(length), C.byref(data), C.byref(item))
    try:
        if value is None:
            if status:
                raise RuntimeError(f'Keychain read failed ({status}); run sign-in first')
            return C.string_at(data, length.value).decode()
        value = value.encode()
        if status == -25300:  # errSecItemNotFound
            status = security.SecKeychainAddGenericPassword(None, len(SERVICE), SERVICE,
                len(account), account, len(value), value, None)
        elif status == 0:
            status = security.SecKeychainItemModifyAttributesAndData(item, None, len(value), value)
        if status:
            raise RuntimeError(f'Keychain write failed ({status})')
    finally:
        if data:
            security.SecKeychainItemFreeContent(None, data)
        if item:
            core.CFRelease(item)
