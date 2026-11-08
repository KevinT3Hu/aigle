#include "platform.hpp"
#include "commands.hpp"
#include "aigle.h"
#include "theme.hpp"
#include "localization.hpp"
#include "window.hpp"
#include <QApplication>
#include <QCommandLineParser>
#include <QMessageBox>
#include <QVulkanInstance>
#include <rhi/qrhi_platform.h>

int main(int argc, char** argv) {
    aigle_initialize(); // A parser worker must branch before loading any GUI infrastructure.
    aigle::platform::configureDesktop();
    QApplication app(argc, argv);
    QApplication::setApplicationName("Aigle");
    QApplication::setOrganizationName("Aigle");
    QApplication::setApplicationVersion("0.1.0");
    aigle::installTranslations(app);
    aigle::applyTheme(app);
    qRegisterMetaType<aigle::Frame>();
    QCommandLineParser parser;
    parser.setApplicationDescription("Aigle Qt 6 photo editor");
    parser.addHelpOption();
    parser.addVersionOption();
    parser.addPositionalArgument("photo-or-folder", "Photo or folder to open");
    parser.addOption({"synthetic", "Open a generated color chart"});
    parser.addOption({"smoke-test", "Exercise rendering, edit, undo and redo, then exit"});
    parser.addOption({"hdr", "Request an HDR canvas"});
    parser.addOption({"remote",
                      "Check out PHOTO_ID from a Thesauros server (THESAUROS_TOKEN required)",
                      "SERVER"});
    parser.addOption(
        {"hdr-probe", "Open a photo with HDR enabled and exit after 30 seconds", "PHOTO"});
    parser.addOption(
        {"capture-layout", "Capture native layout for visual regression review and exit", "PNG"});
    parser.addOption({"capture-state", "Layout state: develop, library, library-filters, library-searches, export-jobs, comparison, copy-adjustments, preferences, shortcuts, crop, crop-rotated, crop-rotated-zoom, masks, retouch, retouch-eye, agent, export, export-live, scope-waveform, scope-parade, scope-vectorscope, scope-region",
                      "STATE", "develop"});
    parser.addOption(
        {"capture-size", "Logical size for visual review, e.g. 1024x700", "SIZE", "1440x900"});
    parser.process(app);
    QVulkanInstance instance;
    auto extensions = QRhiVulkanInitParams::preferredInstanceExtensions();
    // Surface formats with HDR color spaces are exposed through this extension.
    if (instance.supportedExtensions().contains("VK_EXT_swapchain_colorspace"))
        extensions.append("VK_EXT_swapchain_colorspace");
    instance.setExtensions(extensions);
    instance.setApiVersion(QVersionNumber(1, 1));
    if (!instance.create()) {
        if (parser.isSet("smoke-test")) {
            qCritical("Vulkan is unavailable");
            return 2;
        }
        QMessageBox::critical(
            nullptr, QObject::tr("Aigle"),
            QObject::tr("A Vulkan-capable graphics driver is required for the image canvas."));
        return 2;
    }
    const auto paths = parser.positionalArguments();
    if (parser.isSet("remote") && paths.size() != 1)
        parser.showHelp(2);
    const std::optional<aigle::EngineCommand> initialCommand = parser.isSet("remote")
                                           ? std::optional<aigle::EngineCommand>{aigle::commands::RemoteOpen{.server = parser.value("remote"), .id = paths.first()}}
                                           : std::nullopt;
    aigle::Window window(&instance,
                         parser.isSet("hdr-probe") ? parser.value("hdr-probe")
                                                   : (paths.isEmpty() ? QString{} : paths.first()),
                         parser.isSet("synthetic"), parser.isSet("smoke-test"), initialCommand);
    if (parser.isSet("hdr") || parser.isSet("hdr-probe")) window.setHdr(true);
    if (parser.isSet("hdr-probe"))
        QTimer::singleShot(30000, &window, &QWidget::close);
    QObject::connect(&window, &aigle::Window::smokeFinished, &app,
                     [&app](int code) { app.exit(code); });
    if (parser.isSet("capture-layout")) {
        const auto parts = parser.value("capture-size").split('x');
        if (parts.size() == 2)
            window.setFixedSize(parts[0].toInt(), parts[1].toInt());
        QTimer::singleShot(5000, &window, [&window, &parser] {
            window.captureLayout(parser.value("capture-layout"), parser.value("capture-state"));
        });
        QTimer::singleShot(30000, &app, [&app] {
            qCritical("Layout capture timed out");
            app.exit(2);
        });
    }
    window.show();
    return app.exec();
}
