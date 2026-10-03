// Independently implemented against Windows SDK XAML Diagnostics interfaces.
// No third-party taskbar implementation is included in this component.
#include <windows.h>
#include <sddl.h>
#include <tlhelp32.h>
#include <xamlom.h>
#undef GetCurrentTime
#include <winrt/base.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.UI.Core.h>
#include <winrt/Windows.System.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include <winrt/Windows.UI.Xaml.Shapes.h>
#include <atomic>
#include <algorithm>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

namespace xaml = winrt::Windows::UI::Xaml;
namespace media = winrt::Windows::UI::Xaml::Media;
namespace core = winrt::Windows::UI::Core;
constexpr DWORD kMagic = 0x43575432;
// Fresh class ID for this component, unrelated to other taskbar tools.
const CLSID kTapClass = {0xd1a5b30b,0x123a,0x4e26,{0x97,0x39,0x9c,0xe2,0x75,0x4e,0xd4,0x6b}};
struct Snapshot { DWORD state; HRESULT error; DWORD backgrounds; DWORD explorer; };
enum : DWORD { kModeTransparent = 0, kModeAcrylic = 1, kModeSolid = 2 };
struct AppearanceOptions { DWORD mode; DWORD opacity; DWORD tint; LONG showBorder; };
struct Wire { DWORD magic; DWORD owner; volatile LONG enabled; Snapshot snapshot; AppearanceOptions options; volatile LONG optionsRevision; volatile LONG attached; };
static_assert(sizeof(AppearanceOptions) == 16);
static_assert(sizeof(Snapshot) == 16);
HMODULE gModule;
UINT gAttachMessage;
std::atomic_bool gDiagnosticsStarted{false};

struct Handle {
    HANDLE value{};
    explicit Handle(HANDLE h = nullptr):value(h){}
    ~Handle(){if(value && value != INVALID_HANDLE_VALUE) CloseHandle(value);}
};
std::wstring UserScope() {
    HANDLE token{};
    if(!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) winrt::throw_last_error();
    Handle close(token);
    DWORD size{}; GetTokenInformation(token, TokenUser, nullptr, 0, &size);
    std::vector<BYTE> bytes(size);
    if(!GetTokenInformation(token, TokenUser, bytes.data(), size, &size)) winrt::throw_last_error();
    LPWSTR sid{};
    if(!ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(bytes.data())->User.Sid, &sid)) winrt::throw_last_error();
    std::wstring result(sid); LocalFree(sid); return result;
}
std::wstring MapName(DWORD pid) { return L"Local\\ConvenientWindow.Taskbar.v2." + UserScope() + L"." + std::to_wstring(pid); }
struct Owner {
    Handle mapping, process;
    Wire* wire{};
    ~Owner(){if(wire) UnmapViewOfFile(wire);}
};
std::mutex gOwnerLock;
std::shared_ptr<Owner> gOwner;
HANDLE gOwnerChanged{};
std::shared_ptr<Owner> CurrentOwner(){std::scoped_lock lock(gOwnerLock);return gOwner;}
// The helper is the single writer. A bounded seqlock read keeps mode, tint and
// strength coherent without ever waiting indefinitely on Explorer's UI thread.
bool ReadOptions(const std::shared_ptr<Owner>& owner, AppearanceOptions& options) {
    if(!owner) return false;
    auto wire = owner->wire;
    for(unsigned attempt=0;attempt<3;++attempt) {
        const LONG before = InterlockedCompareExchange(&wire->optionsRevision,0,0);
        if(before & 1) continue;
        options.mode = static_cast<DWORD>(InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(&wire->options.mode),0,0));
        options.opacity = static_cast<DWORD>(InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(&wire->options.opacity),0,0));
        options.tint = static_cast<DWORD>(InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(&wire->options.tint),0,0));
        options.showBorder = InterlockedCompareExchange(&wire->options.showBorder,0,0);
        if(before == InterlockedCompareExchange(&wire->optionsRevision,0,0)) return true;
    }
    return false;
}
void Report(const std::shared_ptr<Owner>& owner, DWORD state, HRESULT error, DWORD count=0) {
    if(!owner) return;
    owner->wire->snapshot.error=error;
    owner->wire->snapshot.backgrounds=count;
    owner->wire->snapshot.explorer=GetCurrentProcessId();
    // Publish state last so readers do not treat an in-progress write as ready.
    InterlockedExchange(reinterpret_cast<volatile LONG*>(&owner->wire->snapshot.state),state);
}

struct Entry {
    xaml::Shapes::Rectangle rectangle{nullptr};
    media::Brush original{nullptr};
    core::CoreDispatcher dispatcher{nullptr};
    winrt::Windows::System::DispatcherQueue queue{nullptr};
    std::atomic_bool ready{false};
    std::atomic_bool removed{false};
    int64_t fillCallback{};
    bool changing{}; // Accessed only on the element's UI thread.
    bool background{};
};
std::mutex gEntriesLock;
std::unordered_map<InstanceHandle,std::shared_ptr<Entry>> gEntries;
std::atomic<DWORD> gGeneration{0};
winrt::Windows::UI::Color TintColor(const AppearanceOptions& options) {
    auto opacity = std::min<DWORD>(options.opacity, 100);
    auto alpha = static_cast<BYTE>((opacity * 255u + 50u) / 100u);
    return winrt::Windows::UI::Color{
        alpha,
        static_cast<BYTE>((options.tint >> 16) & 0xFF),
        static_cast<BYTE>((options.tint >> 8) & 0xFF),
        static_cast<BYTE>(options.tint & 0xFF)
    };
}
media::Brush BackgroundBrush(const AppearanceOptions& options) {
    if(options.mode == kModeTransparent) {
        return media::SolidColorBrush(winrt::Windows::UI::Color{0,0,0,0});
    }
    auto color = TintColor(options);
    if(options.mode == kModeAcrylic) {
        // Alpha is controlled only by TintOpacity, not twice via TintColor.A.
        color.A = 255;
        media::AcrylicBrush acrylic;
        acrylic.BackgroundSource(media::AcrylicBackgroundSource::Backdrop);
        acrylic.TintColor(color);
        acrylic.TintOpacity(static_cast<double>(std::min<DWORD>(options.opacity, 100)) / 100.0);
        acrylic.FallbackColor(color);
        return acrylic;
    }
    return media::SolidColorBrush(color);
}
bool SameOptions(const AppearanceOptions& left, const AppearanceOptions& right) {
    return left.mode == right.mode && left.opacity == right.opacity && left.tint == right.tint && left.showBorder == right.showBorder;
}
void Apply(bool active, const AppearanceOptions& options, const std::shared_ptr<Owner>& owner) {
    std::vector<std::shared_ptr<Entry>> entries;
    {std::scoped_lock lock(gEntriesLock);for(auto& [id,entry]:gEntries) if(entry->ready && !entry->removed) entries.push_back(entry);}
    if(entries.empty()) return;
    auto pending=std::make_shared<std::atomic<DWORD>>(static_cast<DWORD>(entries.size()));
    auto failed=std::make_shared<std::atomic<HRESULT>>(S_OK);
    auto count=static_cast<DWORD>(std::count_if(entries.begin(),entries.end(),[](auto& e){return e->background;}));
    if(active && count==0) return;
    auto generation=++gGeneration;
    for(auto& entry:entries) {
        auto work=[entry,active,options,owner,pending,failed,count,generation] {
            try {
                // An obsolete queued operation must never overwrite a newer restore/apply.
                if(generation==gGeneration.load() && !entry->removed) {
                    struct ChangeGuard { bool& value; ChangeGuard(bool& v):value(v){value=true;} ~ChangeGuard(){value=false;} } guard(entry->changing);
                    if(!active) {
                        entry->rectangle.Fill(entry->original);
                    } else if(entry->background) {
                        entry->rectangle.Fill(BackgroundBrush(options));
                    } else if(options.showBorder) {
                        entry->rectangle.Fill(entry->original);
                    } else {
                        media::SolidColorBrush clear(winrt::Windows::UI::Color{0,0,0,0});
                        entry->rectangle.Fill(clear);
                    }
                }
            } catch(...) {failed->store(winrt::to_hresult());}
            if(--*pending==0 && generation==gGeneration.load()) {
                auto hr=failed->load();
                Report(owner,FAILED(hr)?6:(active?2:0),hr,count);
            }
        };
        try {
            if(entry->queue) winrt::check_bool(entry->queue.TryEnqueue(work));
            else if(entry->dispatcher) entry->dispatcher.RunAsync(core::CoreDispatcherPriority::Normal,work);
            else winrt::throw_hresult(E_UNEXPECTED);
        }
        catch(...) {failed->store(winrt::to_hresult());if(--*pending==0) Report(owner,6,failed->load(),count);}
    }
}

void ApplyCurrent(const std::shared_ptr<Owner>& owner) {
    AppearanceOptions options{};
    if(ReadOptions(owner,options)) Apply(true,options,owner);
}

struct TreeWatcher : winrt::implements<TreeWatcher,IVisualTreeServiceCallback2,winrt::non_agile> {
    winrt::com_ptr<IXamlDiagnostics> diagnostics;
    winrt::com_ptr<IVisualTreeService3> service;
    explicit TreeWatcher(IUnknown* site) {
        winrt::check_hresult(site->QueryInterface(diagnostics.put()));
        winrt::check_hresult(site->QueryInterface(service.put()));
    }
    HRESULT __stdcall OnVisualTreeChange(ParentChildRelation, VisualElement element, VisualMutationType mutation) noexcept override {
        try {
            if(mutation==Remove) {
                std::shared_ptr<Entry> removed;
                {std::scoped_lock lock(gEntriesLock);auto found=gEntries.find(element.Handle);if(found!=gEntries.end()){removed=found->second;gEntries.erase(found);}}
                if(removed) {
                    removed->removed=true;
                    try {removed->rectangle.UnregisterPropertyChangedCallback(xaml::Shapes::Shape::FillProperty(),removed->fillCallback);} catch(...) {}
                }
                return S_OK;
            }
            if(mutation!=Add || !element.Name) return S_OK;
            std::wstring_view name(element.Name);
            if(name!=L"BackgroundFill" && name!=L"BackgroundStroke") return S_OK;
            if(!element.Type || std::wstring_view(element.Type)!=L"Windows.UI.Xaml.Shapes.Rectangle") return S_OK;
            winrt::com_ptr<::IInspectable> inspectable;
            winrt::check_hresult(diagnostics->GetIInspectableFromHandle(element.Handle, reinterpret_cast<::IInspectable**>(inspectable.put())));
            auto rectangle=inspectable.as<xaml::Shapes::Rectangle>();
            // Names alone are not enough: reject unrelated XAML backgrounds.
            auto parent=rectangle.as<xaml::DependencyObject>();
            bool taskbar=false;
            for(unsigned depth=0;parent && depth<24;++depth) {
                if(winrt::get_class_name(parent)==L"Taskbar.TaskbarFrame") {taskbar=true;break;}
                parent=media::VisualTreeHelper::GetParent(parent);
            }
            if(!taskbar) return S_OK;
            auto entry=std::make_shared<Entry>();
            entry->rectangle=rectangle;entry->original=rectangle.Fill();
            entry->dispatcher=rectangle.Dispatcher();entry->background=name==L"BackgroundFill";
            entry->queue=winrt::Windows::System::DispatcherQueue::GetForCurrentThread();
            entry->ready=entry->original!=nullptr;
            {std::scoped_lock lock(gEntriesLock);if(!gEntries.try_emplace(element.Handle,entry).second) return S_OK;}
            std::weak_ptr<Entry> weakEntry(entry);
            entry->fillCallback=rectangle.RegisterPropertyChangedCallback(xaml::Shapes::Shape::FillProperty(),[weakEntry](auto const&, auto const&) {
                try {
                    auto current=weakEntry.lock();
                    if(!current || current->changing || current->removed) return;
                    auto updated=current->rectangle.Fill();
                    if(updated) {current->original=updated;current->ready=true;}
                    auto owner=CurrentOwner();
                    if(owner && owner->wire->enabled>0 && current->ready) ApplyCurrent(owner);
                } catch(...) {Report(CurrentOwner(),6,winrt::to_hresult());}
            });
            auto owner=CurrentOwner();
            if(owner && owner->wire->enabled>0) ApplyCurrent(owner);
            return S_OK;
        } catch(...) {auto hr=winrt::to_hresult();Report(CurrentOwner(),6,hr);return hr;}
    }
    HRESULT __stdcall OnElementStateChanged(InstanceHandle, VisualElementState, LPCWSTR) noexcept override {return S_OK;}
};
winrt::com_ptr<TreeWatcher> gWatcher;
struct TapSite : winrt::implements<TapSite,IObjectWithSite> {
    winrt::com_ptr<IUnknown> site;
    HRESULT __stdcall SetSite(IUnknown* value) noexcept override {
        try {
            if(!value) return S_OK;
            site.copy_from(value);
            if(!gWatcher) {
                gWatcher=winrt::make_self<TreeWatcher>(value);
                auto watcher=gWatcher;
                // Diagnostics can call back during subscription; keep that work outside SetSite.
                std::thread([watcher]{
                    try {
                        winrt::init_apartment(winrt::apartment_type::multi_threaded);
                        auto hr=watcher->service->AdviseVisualTreeChange(watcher.get());
                        if(FAILED(hr)) Report(CurrentOwner(),6,hr);
                    } catch(...) {Report(CurrentOwner(),6,winrt::to_hresult());}
                }).detach();
            }
            return S_OK;
        } catch(...) {auto hr=winrt::to_hresult();Report(CurrentOwner(),6,hr);return hr;}
    }
    HRESULT __stdcall GetSite(REFIID iid,void** value) noexcept override {return site?site->QueryInterface(iid,value):E_FAIL;}
};
struct Factory : winrt::implements<Factory,IClassFactory> {
    HRESULT __stdcall CreateInstance(IUnknown* outer,REFIID iid,void** value) noexcept override {
        if(outer) return CLASS_E_NOAGGREGATION;
        try {return winrt::make_self<TapSite>()->QueryInterface(iid,value);} catch(...) {return winrt::to_hresult();}
    }
    HRESULT __stdcall LockServer(BOOL) noexcept override {return S_OK;}
};
STDAPI DllGetClassObject(REFCLSID clsid,REFIID iid,void** value) {
    if(clsid!=kTapClass) return CLASS_E_CLASSNOTAVAILABLE;
    try {return winrt::make_self<Factory>()->QueryInterface(iid,value);} catch(...) {return winrt::to_hresult();}
}
STDAPI DllCanUnloadNow() {return S_FALSE;}

void InitializeDiagnostics() {
    if(gDiagnosticsStarted.exchange(true)) return;
    std::thread([]{
        try {
            winrt::init_apartment(winrt::apartment_type::multi_threaded);
            auto xamlDll=LoadLibraryExW(L"Windows.UI.Xaml.dll",nullptr,LOAD_LIBRARY_SEARCH_SYSTEM32);
            if(!xamlDll) {Report(CurrentOwner(),6,HRESULT_FROM_WIN32(GetLastError()));return;}
            auto initialize=reinterpret_cast<decltype(&InitializeXamlDiagnosticsEx)>(GetProcAddress(xamlDll,"InitializeXamlDiagnosticsEx"));
            std::wstring location(32768,L'\0');
            auto length=GetModuleFileNameW(gModule,location.data(),static_cast<DWORD>(location.size()));
            location.resize(length);
            HRESULT result=E_NOINTERFACE;
            if(initialize && length) {
                for(unsigned endpoint=1;endpoint<=20;++endpoint) {
                    auto owner=CurrentOwner();
                    if(!owner || owner->wire->enabled<=0) break;
                    std::thread attempt([&]{
                        auto name=L"VisualDiagConnection"+std::to_wstring(endpoint);
                        result=initialize(name.c_str(),GetCurrentProcessId(),nullptr,location.c_str(),kTapClass,nullptr);
                    });
                    attempt.join();
                    if(SUCCEEDED(result)) break;
                    Sleep(200);
                }
            }
            FreeLibrary(xamlDll);
            if(FAILED(result)) {Report(CurrentOwner(),6,result);gDiagnosticsStarted=false;}
        } catch(...) {Report(CurrentOwner(),6,winrt::to_hresult());gDiagnosticsStarted=false;}
    }).detach();
}
void MonitorOwner() {
    for(;;) {
        auto owner=CurrentOwner();
        if(!owner) {WaitForSingleObject(gOwnerChanged,INFINITE);continue;}
        LONG last=-2; auto start=GetTickCount64(); DWORD previousCount=0; AppearanceOptions previousOptions{};
        for(;;) {
            auto current=CurrentOwner();if(current!=owner) break;
            bool dead=WaitForSingleObject(owner->process.value,200)==WAIT_OBJECT_0;
            LONG desired=dead?0:InterlockedCompareExchange(&owner->wire->enabled,0,0);
            DWORD count=0;{std::scoped_lock lock(gEntriesLock);for(auto& [id,e]:gEntries) if(e->background && e->ready && !e->removed) ++count;}
            AppearanceOptions options{};
            if(desired>0 && !ReadOptions(owner,options)) continue;
            const bool optionsChanged = !SameOptions(previousOptions,options);
            if(desired!=last || count!=previousCount || optionsChanged) {
                if(count) Apply(desired>0,options,owner);
                else if(desired<=0) Report(owner,0,S_OK);
                last=desired;previousCount=count;previousOptions=options;
            }
            if(desired>0 && !count && GetTickCount64()-start>10000 && owner->wire->snapshot.state==1) Report(owner,5,HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED));
            if(dead || desired<0) {
                if(count) Apply(false,{},owner);
                std::scoped_lock lock(gOwnerLock);if(gOwner==owner) gOwner.reset();break;
            }
        }
    }
}
extern "C" __declspec(dllexport) LRESULT CALLBACK TaskbarHook(int code,WPARAM wparam,LPARAM lparam) noexcept {
    if(code>=0) {
        if(!gAttachMessage) gAttachMessage=RegisterWindowMessageW(L"ConvenientWindow.Taskbar.Attach.v2");
        auto message=reinterpret_cast<CWPSTRUCT*>(lparam);
        if(message && message->message==gAttachMessage) {
            try {
                auto pid=static_cast<DWORD>(message->wParam);
                auto owner=std::make_shared<Owner>();
                owner->mapping.value=OpenFileMappingW(FILE_MAP_ALL_ACCESS,FALSE,MapName(pid).c_str());
                if(owner->mapping.value) owner->wire=static_cast<Wire*>(MapViewOfFile(owner->mapping.value,FILE_MAP_ALL_ACCESS,0,0,sizeof(Wire)));
                if(owner->wire && owner->wire->magic==kMagic && owner->wire->owner==pid) {
                    auto installed=GetPropW(message->hwnd,L"ConvenientWindow.Taskbar.Module.v1");
                    if(installed && installed!=reinterpret_cast<HANDLE>(gModule)) {
                        Report(owner,6,HRESULT_FROM_WIN32(ERROR_PRODUCT_VERSION));
                        InterlockedExchange(&owner->wire->attached,1);
                        return CallNextHookEx(nullptr,code,wparam,lparam);
                    }
                    owner->process.value=OpenProcess(SYNCHRONIZE,FALSE,pid);
                    if(owner->process.value) {
                        auto old=CurrentOwner();
                        // Another live controller must never take over this injected module.
                        if(!old || old->wire->owner==pid || WaitForSingleObject(old->process.value,0)==WAIT_OBJECT_0) {
                            // Our detached watchers must remain executable even if Diagnostics fails.
                            // Pin only the Explorer copy, never the helper controller copy.
                            HMODULE pinned{};
                            if(!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, reinterpret_cast<LPCWSTR>(&TaskbarHook), &pinned)) return CallNextHookEx(nullptr,code,wparam,lparam);
                            if(!SetPropW(message->hwnd,L"ConvenientWindow.Taskbar.Module.v1",reinterpret_cast<HANDLE>(gModule))) {
                                Report(owner,6,HRESULT_FROM_WIN32(GetLastError()));
                                InterlockedExchange(&owner->wire->attached,1);
                                return CallNextHookEx(nullptr,code,wparam,lparam);
                            }
                            {std::scoped_lock lock(gOwnerLock);gOwner=owner;}
                            if(!gOwnerChanged) {gOwnerChanged=CreateEventW(nullptr,FALSE,FALSE,nullptr);std::thread([]{try {MonitorOwner();} catch(...) {auto current=CurrentOwner();if(current){Apply(false,{},current);Report(current,6,winrt::to_hresult());}}}).detach();}
                            InterlockedExchange(&owner->wire->attached,1);
                            SetEvent(gOwnerChanged);InitializeDiagnostics();
                        } else {Report(owner,4,HRESULT_FROM_WIN32(ERROR_BUSY));InterlockedExchange(&owner->wire->attached,1);}
                    } else {Report(owner,6,HRESULT_FROM_WIN32(GetLastError()));InterlockedExchange(&owner->wire->attached,1);
                    }
                }
            } catch(...) {}
        }
    }
    return CallNextHookEx(nullptr,code,wparam,lparam);
}

bool IsModernWindows() {
    using VersionFunction=LONG(WINAPI*)(OSVERSIONINFOW*);
    auto ntdll=GetModuleHandleW(L"ntdll.dll");
    auto versionFunction=reinterpret_cast<VersionFunction>(GetProcAddress(ntdll,"RtlGetVersion"));
    OSVERSIONINFOW version{};version.dwOSVersionInfoSize=sizeof(version);
    return versionFunction && versionFunction(&version)==0 && version.dwMajorVersion>=10 && version.dwBuildNumber>=22621;
}
bool HasOtherTaskbarTool() {
    Handle snapshot(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS,0));
    if(snapshot.value==INVALID_HANDLE_VALUE) return false;
    PROCESSENTRY32W entry{};entry.dwSize=sizeof(entry);
    if(Process32FirstW(snapshot.value,&entry)) do {
        if(_wcsicmp(entry.szExeFile,L"TranslucentTB.exe")==0 || _wcsicmp(entry.szExeFile,L"windhawk.exe")==0) return true;
    } while(Process32NextW(snapshot.value,&entry));
    return false;
}
// Controller ABI, called only by the helper's dedicated worker thread.
Handle gLock,gMapping;
Wire* gWire{};HHOOK gHook{};DWORD gExplorer{};
void CloseController() {
    if(gWire){UnmapViewOfFile(gWire);gWire=nullptr;}
    if(gHook){UnhookWindowsHookEx(gHook);gHook=nullptr;}
    if(gMapping.value){CloseHandle(gMapping.value);gMapping.value=nullptr;}
    if(gLock.value){CloseHandle(gLock.value);gLock.value=nullptr;}
    gExplorer=0;
}
extern "C" __declspec(dllexport) void __cdecl CWTaskbarClose() noexcept {
    if(gWire) {
        InterlockedExchange(&gWire->enabled,-1);
        for(unsigned i=0;i<50 && gWire->snapshot.state!=0;++i) Sleep(20);
    }
    CloseController();
}
extern "C" __declspec(dllexport) void __cdecl CWTaskbarUpdate(BOOL enabled,const AppearanceOptions* requestedOptions,Snapshot* result) noexcept {
    if(!result) return;
    *result={0,S_OK,0,0};
    const AppearanceOptions requested = requestedOptions ? *requestedOptions : AppearanceOptions{kModeTransparent,0,0x233A63,1};
    try {
        if(!enabled && !gWire) return;
        if(!gAttachMessage) gAttachMessage=RegisterWindowMessageW(L"ConvenientWindow.Taskbar.Attach.v2");
        if(!gAttachMessage) winrt::throw_last_error();
        HWND taskbar=FindWindowW(L"Shell_TrayWnd",nullptr);
        DWORD pid=0;DWORD thread=taskbar?GetWindowThreadProcessId(taskbar,&pid):0;
        if(!thread){*result={enabled?1u:0u,S_OK,0,0};return;}
        if(gWire && pid!=gExplorer) CWTaskbarClose();
        if(!gWire && enabled) {
            if(!IsModernWindows()) {*result={5,HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED),0,pid};return;}
            if(HasOtherTaskbarTool()) {*result={4,HRESULT_FROM_WIN32(ERROR_BUSY),0,pid};return;}
            auto lockName=L"Local\\ConvenientWindow.Taskbar.Owner.v1."+UserScope();
            gLock.value=CreateMutexW(nullptr,FALSE,lockName.c_str());
            if(!gLock.value) winrt::throw_last_error();
            if(GetLastError()==ERROR_ALREADY_EXISTS) {CloseController();*result={4,HRESULT_FROM_WIN32(ERROR_BUSY),0,pid};return;}
            gMapping.value=CreateFileMappingW(INVALID_HANDLE_VALUE,nullptr,PAGE_READWRITE,0,sizeof(Wire),MapName(GetCurrentProcessId()).c_str());
            if(!gMapping.value) winrt::throw_last_error();
            gWire=static_cast<Wire*>(MapViewOfFile(gMapping.value,FILE_MAP_ALL_ACCESS,0,0,sizeof(Wire)));
            if(!gWire) winrt::throw_last_error();
            *gWire={kMagic,GetCurrentProcessId(),1,{1,S_OK,0,pid},requested,2,0};gExplorer=pid;
            gHook=SetWindowsHookExW(WH_CALLWNDPROC,TaskbarHook,gModule,thread);
            if(!gHook) winrt::throw_last_error();
            DWORD_PTR ignored{};
            if(!SendMessageTimeoutW(taskbar,gAttachMessage,GetCurrentProcessId(),0,SMTO_ABORTIFHUNG,1000,&ignored)) winrt::throw_last_error();
            if(gWire->attached!=1) winrt::throw_hresult(HRESULT_FROM_WIN32(ERROR_DLL_INIT_FAILED));
        }
        if(gWire) {
            if(!SameOptions(gWire->options,requested)) {
                if(enabled) InterlockedExchange(reinterpret_cast<volatile LONG*>(&gWire->snapshot.state),1);
                InterlockedIncrement(&gWire->optionsRevision);
                InterlockedExchange(reinterpret_cast<volatile LONG*>(&gWire->options.mode),static_cast<LONG>(requested.mode));
                InterlockedExchange(reinterpret_cast<volatile LONG*>(&gWire->options.opacity),static_cast<LONG>(requested.opacity));
                InterlockedExchange(reinterpret_cast<volatile LONG*>(&gWire->options.tint),static_cast<LONG>(requested.tint));
                InterlockedExchange(&gWire->options.showBorder,requested.showBorder);
                InterlockedIncrement(&gWire->optionsRevision);
            }
            if(!enabled) {
                const bool detached = gWire->snapshot.state==4 || gWire->snapshot.error==HRESULT_FROM_WIN32(ERROR_PRODUCT_VERSION);
                if(!detached && gWire->enabled>0) InterlockedExchange(reinterpret_cast<volatile LONG*>(&gWire->snapshot.state),3);
                InterlockedExchange(&gWire->enabled,-1);
                // No state is reported restored until the target UI queue acknowledges it.
                for(unsigned i=0;!detached && i<50 && gWire->snapshot.state==3;++i) Sleep(20);
                *result=gWire->snapshot;
                if(detached) *result={0,S_OK,0,0};
                else if(result->state==3) *result={6,HRESULT_FROM_WIN32(WAIT_TIMEOUT),result->backgrounds,pid};
                if(result->state==0 || detached) CloseController();
            } else *result=gWire->snapshot;
        }
    } catch(...) {auto hr=winrt::to_hresult();CWTaskbarClose();*result={6,hr,0,0};}
}
BOOL WINAPI DllMain(HINSTANCE instance,DWORD reason,LPVOID) {
    if(reason==DLL_PROCESS_ATTACH){gModule=instance;DisableThreadLibraryCalls(instance);}
    return TRUE;
}
