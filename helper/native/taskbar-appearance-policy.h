#pragma once
#include <windows.h>
#include <string_view>

// Policy seams used by the real hook and native regressions. The named mutex
// and resident process handle remain the live-owner authority.
namespace TaskbarModulePolicy {
inline HRESULT ProductVersionConflict() noexcept {return HRESULT_FROM_WIN32(ERROR_PRODUCT_VERSION);}
inline bool HasForeignModule(HANDLE installed,HMODULE current) noexcept {
    return installed && installed!=reinterpret_cast<HANDLE>(current);
}
inline bool ShouldClearStaleMarker(HANDLE installed,HMODULE current,bool residentLoaded) noexcept {
    return HasForeignModule(installed,current) && !residentLoaded;
}
inline bool IsLegacyV2Name(std::wstring_view leaf) noexcept {
    if(leaf.size()!=28 || leaf.substr(0,8)!=L"taskbar-" || leaf.substr(24)!=L".dll") return false;
    for(auto c:leaf.substr(8,16)) if(!((c>=L'0' && c<=L'9') || (c>=L'a' && c<=L'f'))) return false;
    return true;
}
} // namespace TaskbarModulePolicy
