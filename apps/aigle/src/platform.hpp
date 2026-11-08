#pragma once
#include <QtGlobal>
#include <QByteArray>

namespace aigle::platform {
inline void configureDesktop() {
#if defined(Q_OS_LINUX)
    if (qEnvironmentVariableIsEmpty("QT_QPA_PLATFORMTHEME"))
        qputenv("QT_QPA_PLATFORMTHEME", "xdgdesktopportal");
#elif defined(Q_OS_WIN)
    // TODO(windows): validate native desktop integration and packaging.
    // Qt's native file dialogs need no portal environment override.
#endif
}
} // namespace aigle::platform
