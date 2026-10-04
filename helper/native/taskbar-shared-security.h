#pragma once
#include <windows.h>
#include <sddl.h>
#include <string>

namespace TaskbarModulePolicy {
// The Explorer-side component writes acknowledgements into this mapping.
// Pin its integrity label to medium even when the controller is elevated,
// while granting access only to the exact current user and SYSTEM. Neither
// the helper process token nor its executable permissions are changed.
class UserScopedMediumSecurity {
    PSECURITY_DESCRIPTOR descriptor_{};
    SECURITY_ATTRIBUTES attributes_{};
public:
    explicit UserScopedMediumSecurity(const std::wstring& userSid) {
        const auto sddl = L"D:P(A;;GA;;;SY)(A;;GA;;;" + userSid + L")S:(ML;;NW;;;ME)";
        if (ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.c_str(), SDDL_REVISION_1, &descriptor_, nullptr)) {
            attributes_ = { sizeof(SECURITY_ATTRIBUTES), descriptor_, FALSE };
        }
    }
    UserScopedMediumSecurity(const UserScopedMediumSecurity&) = delete;
    UserScopedMediumSecurity& operator=(const UserScopedMediumSecurity&) = delete;
    ~UserScopedMediumSecurity() { if (descriptor_) LocalFree(descriptor_); }
    bool valid() const { return descriptor_ != nullptr; }
    SECURITY_ATTRIBUTES* get() { return &attributes_; }
};
}