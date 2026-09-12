// Windows 11 IExplorerCommand; loaded on demand by the package's COM surrogate.
#include <windows.h>
#include <shlobj.h>
#include <shlwapi.h>
#include <wrl.h>
#include <wrl/module.h>
#include <memory>
#include <string>
#ifdef SHELL_MENU_TEST
#include <cassert>
#endif

using namespace Microsoft::WRL;
static HMODULE module;
using ShellString = std::unique_ptr<wchar_t, decltype(&CoTaskMemFree)>;

static std::wstring executable() {
    std::wstring path(32768, L'\0');
    const DWORD length = GetModuleFileNameW(module, path.data(), static_cast<DWORD>(path.size()));
    if (!length || length >= path.size()) return {};
    path.resize(length);
    const auto slash = path.find_last_of(L'\\');
    return slash == std::wstring::npos ? std::wstring{} : path.substr(0, slash + 1) + L"PlumeTxt.exe";
}

// Filesystem paths cannot contain quotes. Escape trailing slashes for Windows argv,
// including drive roots and UNC roots. No command interpreter handles the path.
static std::wstring argument(const wchar_t* path) {
    std::wstring value(path);
    if (value.empty() || value.find(L'"') != std::wstring::npos) return {};
    const auto end = value.find_last_not_of(L'\\');
    const auto trailing = end == std::wstring::npos ? value.size() : value.size() - end - 1;
    return L"\"" + value + std::wstring(trailing, L'\\') + L"\"";
}

class __declspec(uuid("B8B0A8C5-FB75-4CED-BB82-50E78317A931")) OpenCommand final
    : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IExplorerCommand, IObjectWithSite> {
    ComPtr<IUnknown> site;

    HRESULT item(IShellItemArray* items, IShellItem** result) {
        *result = nullptr;
        DWORD count = 0;
        if (items && SUCCEEDED(items->GetCount(&count)) && count) {
            if (count != 1) return E_INVALIDARG;
            return items->GetItemAt(0, result);
        }
        // A folder-background invocation has no selected item; ask the hosting view.
        if (!site) return E_FAIL;
        ComPtr<IShellBrowser> browser;
        HRESULT hr = IUnknown_QueryService(site.Get(), SID_STopLevelBrowser, IID_PPV_ARGS(&browser));
        if (FAILED(hr)) return hr;
        ComPtr<IShellView> view;
        hr = browser->QueryActiveShellView(&view);
        if (FAILED(hr)) return hr;
        ComPtr<IFolderView> folder;
        hr = view.As(&folder);
        return FAILED(hr) ? hr : folder->GetFolder(IID_PPV_ARGS(result));
    }

public:
    IFACEMETHODIMP GetTitle(IShellItemArray*, PWSTR* title) override {
        return SHStrDupW(PRIMARYLANGID(GetUserDefaultUILanguage()) == LANG_CHINESE
            ? L"\u7528 PlumeTxt \u6253\u5f00" : L"Open with PlumeTxt", title);
    }
    IFACEMETHODIMP GetIcon(IShellItemArray*, PWSTR* icon) override {
        *icon = nullptr;
        try {
            auto path = executable();
            if (path.empty()) return E_FAIL;
            return SHStrDupW((L"\"" + path + L"\",-1").c_str(), icon);
        } catch (const std::bad_alloc&) { return E_OUTOFMEMORY; }
    }
    IFACEMETHODIMP GetToolTip(IShellItemArray*, PWSTR* tip) override {
        *tip = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP GetCanonicalName(GUID* name) override { *name = __uuidof(OpenCommand); return S_OK; }
    IFACEMETHODIMP GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* state) override {
        DWORD count = 0;
        *state = items && (FAILED(items->GetCount(&count)) || count > 1) ? ECS_HIDDEN : ECS_ENABLED;
        return S_OK;
    }
    IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) override {
        ComPtr<IShellItem> selected;
        HRESULT hr = item(items, &selected);
        if (FAILED(hr)) return hr;
        PWSTR raw = nullptr;
        hr = selected->GetDisplayName(SIGDN_FILESYSPATH, &raw);
        if (FAILED(hr)) return hr;
        ShellString path(raw, CoTaskMemFree);
        try {
            auto exe = executable();
            auto args = argument(path.get());
            if (exe.empty() || args.empty()) return E_INVALIDARG;
            auto line = argument(exe.c_str()) + L" " + args;
            STARTUPINFOW startup{sizeof(startup)};
            PROCESS_INFORMATION process{};
            if (!CreateProcessW(exe.c_str(), line.data(), nullptr, nullptr, FALSE, 0,
                nullptr, nullptr, &startup, &process)) return HRESULT_FROM_WIN32(GetLastError());
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
            return S_OK;
        } catch (const std::bad_alloc&) { return E_OUTOFMEMORY; }
    }
    IFACEMETHODIMP GetFlags(EXPCMDFLAGS* flags) override { *flags = ECF_DEFAULT; return S_OK; }
    IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** commands) override {
        *commands = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP SetSite(IUnknown* host) override { site = host; return S_OK; }
    IFACEMETHODIMP GetSite(REFIID iid, void** result) override {
        *result = nullptr;
        return site ? site->QueryInterface(iid, result) : E_FAIL;
    }
};

CoCreatableClass(OpenCommand);
STDAPI DllGetClassObject(REFCLSID clsid, REFIID iid, void** value) {
    return Module<InProc>::GetModule().GetClassObject(clsid, iid, value);
}
STDAPI DllCanUnloadNow() {
    return Module<InProc>::GetModule().GetObjectCount() == 0 ? S_OK : S_FALSE;
}
BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, void*) {
    if (reason == DLL_PROCESS_ATTACH) module = instance;
    return TRUE;
}

#ifdef SHELL_MENU_TEST
int wmain() {
    assert(SUCCEEDED(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED)));
    for (const auto* path : {L"C:\\", L"C:\\space \u4e2d\u6587\\", L"\\\\server\\share\\", L"C:\\a&b'$.md"}) {
        const auto line = L"PlumeTxt.exe " + argument(path);
        int count = 0;
        auto argv = CommandLineToArgvW(line.c_str(), &count);
        assert(argv && count == 2 && std::wstring(argv[1]) == path);
        LocalFree(argv);
    }
    assert(argument(L"bad\"path").empty());
    assert(DllCanUnloadNow() == S_OK);
    {
        ComPtr<IClassFactory> factory;
        assert(SUCCEEDED(DllGetClassObject(__uuidof(OpenCommand), IID_PPV_ARGS(&factory))));
        ComPtr<IExplorerCommand> command;
        assert(SUCCEEDED(factory->CreateInstance(nullptr, IID_PPV_ARGS(&command))));
        assert(DllCanUnloadNow() == S_FALSE);
        PWSTR title = nullptr;
        assert(SUCCEEDED(command->GetTitle(nullptr, &title)) && title && *title);
        CoTaskMemFree(title);
        EXPCMDFLAGS flags;
        assert(SUCCEEDED(command->GetFlags(&flags)) && flags == ECF_DEFAULT);
        assert(FAILED(command->Invoke(nullptr, nullptr)));
    }
    assert(DllCanUnloadNow() == S_OK);
    CoUninitialize();
}
#endif
