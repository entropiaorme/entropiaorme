#pragma once
#include <windows.h>
#include <msiquery.h>
#include <objbase.h>
#include <shlwapi.h>
#include <strsafe.h>
#include <dwmapi.h>

#include "dutil.h"
#include "memutil.h"
#include "dictutil.h"
#include "fileutil.h"
#include "pathutil.h"
#include "strutil.h"
#include "regutil.h"

// BootstrapperApplicationBase.h pulls IBootstrapperApplication + the engine
// types before balutil.h, so include it before anything using Bal*/engine.
#include "BootstrapperApplicationBase.h"
#include "BAFunctions.h"
#include "IBAFunctions.h"
