// EntropiaOrme installer BA functions: a tiny native helper alongside WixStdBA
// whose only job is to flip the bootstrapper window's title bar to dark mode
// (thmutil cannot touch the OS frame). No .NET runtime; native DWM only.
#include "precomp.h"
#include "BalBaseBAFunctions.h"
#include "BalBaseBAFunctionsProc.h"

static HINSTANCE vhInstance = NULL;

class CEoBAFunctions : public CBalBaseBAFunctions
{
public:
    CEoBAFunctions(__in HMODULE hModule) : CBalBaseBAFunctions(hModule) {}
};

// Poll briefly for the bootstrapper window (the caption is the bundle name +
// " Setup"), then enable DWM immersive dark mode on its frame. Runs on a
// short-lived thread so it never blocks BA creation.
static DWORD WINAPI EoDarkenTitleBar(__in LPVOID /*pv*/)
{
    BOOL fDark = TRUE;
    for (int i = 0; i < 200; ++i)
    {
        HWND hWnd = ::FindWindowW(NULL, L"EntropiaOrme Setup");
        if (hWnd)
        {
            ::DwmSetWindowAttribute(hWnd, 20, &fDark, sizeof(fDark)); // DWMWA_USE_IMMERSIVE_DARK_MODE (20H1+)
            ::DwmSetWindowAttribute(hWnd, 19, &fDark, sizeof(fDark)); // pre-20H1 attribute id
            ::SetWindowPos(hWnd, NULL, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
            return 0;
        }
        ::Sleep(50);
    }
    return 0;
}

static HRESULT CreateBAFunctions(
    __in HMODULE hModule,
    __in const BA_FUNCTIONS_CREATE_ARGS* pArgs,
    __inout BA_FUNCTIONS_CREATE_RESULTS* pResults
    )
{
    HRESULT hr = S_OK;
    CEoBAFunctions* pBAFunctions = NULL;

    pBAFunctions = new CEoBAFunctions(hModule);
    ExitOnNull(pBAFunctions, hr, E_OUTOFMEMORY, "Failed to create CEoBAFunctions object.");

    hr = pBAFunctions->OnCreate(pArgs->pEngine, pArgs->pCommand);
    ExitOnFailure(hr, "Failed to call OnCreate CEoBAFunctions.");

    pResults->pfnBAFunctionsProc = BalBaseBAFunctionsProc;
    pResults->pvBAFunctionsProcContext = pBAFunctions;
    pBAFunctions = NULL;

    ::CloseHandle(::CreateThread(NULL, 0, EoDarkenTitleBar, NULL, 0, NULL));

LExit:
    ReleaseObject(pBAFunctions);
    return hr;
}

extern "C" BOOL WINAPI DllMain(
    IN HINSTANCE hInstance,
    IN DWORD dwReason,
    IN LPVOID /*pvReserved*/
    )
{
    switch (dwReason)
    {
    case DLL_PROCESS_ATTACH:
        ::DisableThreadLibraryCalls(hInstance);
        vhInstance = hInstance;
        break;
    case DLL_PROCESS_DETACH:
        vhInstance = NULL;
        break;
    }
    return TRUE;
}

extern "C" HRESULT WINAPI BAFunctionsCreate(
    __in const BA_FUNCTIONS_CREATE_ARGS* pArgs,
    __inout BA_FUNCTIONS_CREATE_RESULTS* pResults
    )
{
    HRESULT hr = S_OK;
    BalInitialize(pArgs->pEngine);

    hr = CreateBAFunctions(vhInstance, pArgs, pResults);
    BalExitOnFailure(hr, "Failed to create BAFunctions interface.");

LExit:
    return hr;
}

extern "C" void WINAPI BAFunctionsDestroy(
    __in const BA_FUNCTIONS_DESTROY_ARGS* /*pArgs*/,
    __inout BA_FUNCTIONS_DESTROY_RESULTS* /*pResults*/
    )
{
    BalUninitialize();
}
