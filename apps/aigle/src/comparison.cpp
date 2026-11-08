#include "comparison.hpp"
#include "dialog_shortcuts.hpp"
#include "engine.hpp"
#include "canvas.hpp"
#include <QCheckBox>
#include <QDialogButtonBox>
#include <QFileInfo>
#include <QGridLayout>
#include <QJsonArray>
#include <QKeyEvent>
#include <QLabel>
#include <QMouseEvent>
#include <QPainter>
#include <QPushButton>
#include <QShortcut>
#include <QScreen>
#include <QSettings>
#include <QTimer>
#include <QUuid>
#include <QVBoxLayout>
#include <QWheelEvent>
namespace aigle {
ComparisonPane::ComparisonPane(QWidget* parent) : QWidget(parent) {
    setMinimumSize(180, 150);
    setFocusPolicy(Qt::StrongFocus);
    setAccessibleName(tr("Comparison photo: scroll or plus/minus to zoom, drag or arrow keys to "
                         "pan, double-click to fit"));
}
void ComparisonPane::paintEvent(QPaintEvent*) {
    QPainter painter(this);
    painter.fillRect(rect(), QColor("#181818"));
    if (image.isNull()) {
        painter.setPen(QColor("#b0b0b0"));
        painter.drawText(rect().adjusted(12, 12, -12, -12), Qt::AlignCenter | Qt::TextWordWrap,
                         message);
    } else {
        painter.setRenderHint(QPainter::SmoothPixmapTransform);
        const auto target = view.imageRect(imageSize(), size());
        if (frameRegion.isEmpty() || extent.isEmpty())
            painter.drawImage(target, image);
        else {
            const double scale = target.width() / extent.width();
            painter.drawImage(QRectF(target.topLeft() + frameRegion.topLeft() * scale,
                                     frameRegion.size() * scale),
                              image);
        }
    }
}
void ComparisonPane::keyPressEvent(QKeyEvent* event) {
    switch (event->key()) {
    case Qt::Key_Plus:
    case Qt::Key_Equal:
        view.zoomAt(1.2, rect().center(), imageSize(), size());
        break;
    case Qt::Key_Minus:
        view.zoomAt(1 / 1.2, rect().center(), imageSize(), size());
        break;
    case Qt::Key_Left:
        view.pan({40, 0}, imageSize(), size());
        break;
    case Qt::Key_Right:
        view.pan({-40, 0}, imageSize(), size());
        break;
    case Qt::Key_Up:
        view.pan({0, 40}, imageSize(), size());
        break;
    case Qt::Key_Down:
        view.pan({0, -40}, imageSize(), size());
        break;
    default:
        QWidget::keyPressEvent(event);
        return;
    }
    update();
    if (navigated)
        navigated(this);
    event->accept();
}
void ComparisonPane::wheelEvent(QWheelEvent* event) {
    if (image.isNull())
        return;
    view.zoomAt(std::pow(1.2, event->angleDelta().y() / 120.0), event->position(), imageSize(),
                size());
    update();
    if (navigated)
        navigated(this);
    event->accept();
}
void ComparisonPane::mousePressEvent(QMouseEvent* event) {
    if (event->button() == Qt::LeftButton) {
        last_ = event->position();
        setCursor(Qt::ClosedHandCursor);
    }
}
void ComparisonPane::mouseMoveEvent(QMouseEvent* event) {
    if (!(event->buttons() & Qt::LeftButton) || image.isNull())
        return;
    view.pan(event->position() - last_, imageSize(), size());
    last_ = event->position();
    update();
    if (navigated)
        navigated(this);
}
void ComparisonPane::mouseReleaseEvent(QMouseEvent*) {
    unsetCursor();
}
void ComparisonPane::mouseDoubleClickEvent(QMouseEvent*) {
    view = {};
    update();
    if (navigated)
        navigated(this);
}
void showComparison(QWidget* parent, Engine* engine, const QList<PhotoIdentity>& paths,
                    std::function<void(const PhotoIdentity&, const QString&)> flag, Canvas* sourceCanvas) {
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Compare selected photos"));
    dialog.resize(1200, 760);
    auto* layout = new QVBoxLayout(&dialog);
    auto* controls = new QHBoxLayout;
    auto* linked = new QCheckBox(QObject::tr("Link zoom and pan"));
    linked->setChecked(true);
    auto* fit = new QPushButton(QObject::tr("Fit all"));
    controls->addWidget(linked);
    controls->addWidget(fit);
    controls->addStretch();
    const bool native = sourceCanvas && sourceCanvas->vulkanInstance();
    controls->addWidget(new QLabel(native ? QObject::tr("Edited masters · full-resolution rendering")
                                         : QObject::tr("Edited masters · full-resolution rendering · SDR")));
    auto* hdr = new QCheckBox(QObject::tr("HDR canvas"));
    hdr->setVisible(native);
    hdr->setChecked(native && sourceCanvas->hdrRequested());
    controls->addWidget(hdr);
    layout->addLayout(controls);
    auto* actual = new QPushButton(QObject::tr("Actual pixels (1)"));
    controls->insertWidget(2, actual);
    auto* grid = new QGridLayout;
    layout->addLayout(grid, 1);
    QList<ComparisonPane*> panes;
    QList<QWidget*> cards;
    QList<Canvas*> canvases;
    QList<QLabel*> canvasStatus;
    const auto session = QUuid::createUuid().toString();
    bool synchronizing = false;
    const bool autoDisplay = QSettings().value("preferences/autoDisplay", true).toBool();
    QString requestedScreen;
    const auto detectDisplay = [&](Canvas* canvas) {
        const auto screen = canvas->screen() ? canvas->screen()->name() : QString();
        canvas->setDisplayProfile({}, QObject::tr("sRGB"));
        if (!autoDisplay || requestedScreen == screen) return;
        requestedScreen = screen;
        QMetaObject::invokeMethod(engine, [engine, session, screen] {
            engine->comparisonDisplay(session, screen);
        }, Qt::QueuedConnection);
    };
    int remaining = int(paths.size());
    const auto arrange = [&] {
        for (auto* card : cards)
            grid->removeWidget(card);
        int index = 0;
        for (auto* card : cards)
            if (!card->isHidden()) {
                grid->addWidget(card, index / 2, index % 2);
                ++index;
            }
    };
    for (const auto& value : paths) {
        const auto& identity = value;
        const auto path = identity.path;
        auto* card = new QWidget;
        auto* column = new QVBoxLayout(card);
        column->setContentsMargins(0, 0, 0, 0);
        auto* name = new QLabel(identity.name.isNull() ? QFileInfo(path).fileName() : identity.name);
        name->setToolTip(path);
        column->addWidget(name);
        auto* pane = new ComparisonPane;
        pane->message = QObject::tr("Loading edited preview…");
        pane->setProperty("path", path);
        pane->setProperty("copy", identity.copy);
        pane->setProperty("name", name->text());
        if (native) {
            pane->setParent(card);
            pane->hide();
            auto* canvas = new Canvas(sourceCanvas->vulkanInstance());
            const int index = int(canvases.size());
            canvases.append(canvas);
            canvas->setSurveyMode(true);
            QObject::connect(canvas, &Canvas::surveyCloseRequested, &dialog, &QDialog::reject);
            QObject::connect(canvas, &Canvas::surveyFitRequested, fit, &QPushButton::click);
            QObject::connect(canvas, &Canvas::surveyPixelsRequested, actual, &QPushButton::click);
            canvas->setHdr(hdr->isChecked());
            canvas->setDisplayProfile({}, QObject::tr("sRGB"));
            QObject::connect(canvas, &QWindow::screenChanged, &dialog, [&, canvas] { detectDisplay(canvas); });
            auto* container = QWidget::createWindowContainer(canvas, card);
            container->setMinimumSize(180, 150);
            column->addWidget(container, 1);
            auto* status = new QLabel(pane->message);
            status->setWordWrap(true);
            canvasStatus.append(status);
            column->addWidget(status);
            QObject::connect(canvas, &Canvas::displayStatus, status, &QLabel::setText);
            QObject::connect(hdr, &QCheckBox::toggled, canvas, &Canvas::setHdr);
            QObject::connect(canvas, &Canvas::viewportChanged, &dialog,
                [&, canvas, index](const ViewportRequest& request) {
                    if (index >= cards.size() || cards[index]->isHidden()) return;
                    auto vp = request;
                    QMetaObject::invokeMethod(engine, [engine, session, index, vp] {
                        engine->comparisonViewport(session, index, vp);
                    }, Qt::QueuedConnection);
                    if (!linked->isChecked() || synchronizing) return;
                    synchronizing = true;
                    for (auto* target : canvases)
                        if (target != canvas && target->isVisible()) target->setNavigation(canvas->navigation());
                    synchronizing = false;
                });
        } else column->addWidget(pane, 1);
        auto* actions = new QHBoxLayout;
        for (const auto& action :
             QList<QPair<QString, QString>>{{QObject::tr("Pick"), "picked"},
                                            {QObject::tr("Reject"), "rejected"},
                                            {QObject::tr("Clear flag"), "none"}}) {
            auto* button = new QPushButton(action.first);
            actions->addWidget(button);
            QObject::connect(button, &QPushButton::clicked, &dialog,
                             [&, identity, action] { flag(identity, action.second); });
        }
        auto* remove = new QPushButton(QObject::tr("Remove from comparison"));
        actions->addWidget(remove);
        column->addLayout(actions);
        QObject::connect(remove, &QPushButton::clicked, &dialog, [&, card] {
            card->hide();
            --remaining;
            arrange();
            if (remaining == 0)
                dialog.accept();
        });
        pane->navigated = [&](ComparisonPane* source) {
            if (linked->isChecked())
                for (auto* target : panes)
                    if (target != source)
                        target->setView(source->view);
        };
        panes.append(pane);
        cards.append(card);
        grid->addWidget(card, int(cards.size() - 1) / 2, int(cards.size() - 1) % 2);
    }
    QObject::connect(fit, &QPushButton::clicked, &dialog, [&] {
        for (auto* canvas : canvases) canvas->fit();
        for (auto* pane : panes)
            pane->setView({});
    });
    QObject::connect(linked, &QCheckBox::toggled, &dialog, [&](bool enabled) {
        if (enabled && !canvases.isEmpty()) {
            synchronizing = true;
            Canvas* source = nullptr;
            for (int i = 0; i < canvases.size(); ++i)
                if (!cards[i]->isHidden()) { source = canvases[i]; break; }
            if (source)
                for (int i = 0; i < canvases.size(); ++i)
                    if (!cards[i]->isHidden()) canvases[i]->setNavigation(source->navigation());
            synchronizing = false;
        }
        if (enabled && !panes.isEmpty())
            for (auto* pane : panes)
                pane->setView(panes.first()->view);
    });
    QObject::connect(
        engine, &Engine::reply, &dialog, [&](const EngineReply& reply) {
            if (reply.command != "batch_metadata")
                return;
            for (const auto& metadata : reply.get<BatchResult>().updated) {
                const auto path = metadata.identity.path;
                for (int i = 0; i < panes.size(); ++i)
                    if (panes[i]->property("path").toString() == path &&
                        panes[i]->property("copy").toLongLong() == metadata.identity.copy) {
                        const auto flagValue = metadata.facts.flag;
                        const auto label = flagValue == "picked"     ? QObject::tr("Picked")
                                           : flagValue == "rejected" ? QObject::tr("Rejected")
                                                                     : QObject::tr("No flag");
                        cards[i]->findChild<QLabel*>()->setText(panes[i]->property("name").toString() + " · " +
                                                                label);
                    }
            }
        });
    QObject::connect(engine, &Engine::comparisonDisplayReady, &dialog,
        [&](const QString& id, const DisplayDetection& profile) {
            if (id != session || !autoDisplay || profile.running) return;
            for (auto* canvas : canvases) {
                const auto screen = canvas->screen() ? canvas->screen()->name() : QString();
                if (profile.screen != screen) continue;
                const bool valid = profile.profile.has_value();
                canvas->setDisplayProfile(profile.profile.value_or(DisplayProfile{}),
                    valid ? profile.name : QObject::tr("sRGB"));
            }
        });
    QObject::connect(engine, &Engine::comparisonFrameReady, &dialog,
        [&](const QString& id, int index, Frame frame) {
            if (id == session && index >= 0 && index < canvases.size())
                canvases[index]->setFrame(std::move(frame), false);
        });
    QObject::connect(engine, &Engine::comparisonFrame, &dialog,
                     [&](const QString& id, int index, const QImage& image, const QRectF& region,
                         const QSizeF& extent, const QString& error) {
                         if (id != session || index < 0 || index >= panes.size())
                             return;
                         auto* pane = panes[index];
                         if (!extent.isEmpty()) {
                             pane->extent = extent;
                             if (index < canvases.size()) canvases[index]->setExtent(extent);
                         }
                         if (!image.isNull()) {
                             pane->image = image;
                             pane->frameRegion = region;
                         }
                         if (!error.isEmpty()) {
                             pane->message = error;
                             if (index < canvasStatus.size()) canvasStatus[index]->setText(error);
                         }
                         if (!image.isNull() || !error.isEmpty())
                             pane->update();
                     });
    QObject::connect(actual, &QPushButton::clicked, &dialog, [&] {
        synchronizing = true;
        for (auto* canvas : canvases) canvas->oneToOne();
        synchronizing = false;
        for (auto* pane : panes) {
            if (pane->extent.isEmpty())
                continue;
            auto view = pane->view;
            const double fitScale = std::min(pane->width() / pane->extent.width(),
                                             pane->height() / pane->extent.height());
            view.zoom = 1.0 / (fitScale * pane->devicePixelRatioF());
            pane->setView(view);
        }
    });
    auto* actualKey = new QShortcut(commandShortcut("comparison_pixels", "1"), &dialog);
    QObject::connect(actualKey, &QShortcut::activated, actual, &QPushButton::click);
    QList<std::optional<ViewportRequest>> sent;
    for (int i = 0; i < panes.size(); ++i)
        sent.append(std::nullopt);
    QTimer renderTimer;
    renderTimer.setInterval(80);
    QObject::connect(&renderTimer, &QTimer::timeout, &dialog, [&] {
        for (int i = 0; i < panes.size(); ++i) {
            auto* pane = panes[i];
            if (pane->extent.isEmpty() || cards[i]->isHidden())
                continue;
            const auto target = pane->view.imageRect(pane->extent, pane->size());
            const double scale = target.width() / pane->extent.width();
            const auto region = QRectF(-target.topLeft() / scale, QSizeF(pane->size()) / scale);
            const double dpr = pane->devicePixelRatioF();
            ViewportRequest vp{region,
                {std::clamp(int(pane->width() * dpr), 1, 4096),
                 std::clamp(int(pane->height() * dpr), 1, 4096)}, {}, false};
            if (sent[i] == vp)
                continue;
            sent[i] = vp;
            QMetaObject::invokeMethod(
                engine, [engine, session, i, vp] { engine->comparisonViewport(session, i, vp); },
                Qt::QueuedConnection);
        }
    });
    if (!native) renderTimer.start();
    auto* close = new QDialogButtonBox(QDialogButtonBox::Close);
    layout->addWidget(close);
    QObject::connect(close, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    auto* fitKey = new QShortcut(commandShortcut("comparison_fit", "F"), &dialog);
    QObject::connect(fitKey, &QShortcut::activated, fit, &QPushButton::click);
    QMetaObject::invokeMethod(
        engine, [engine, paths, session] { engine->comparisonStart(paths, session); },
        Qt::QueuedConnection);
    // Queue discovery after session creation, even if construction emitted an early screen event.
    requestedScreen.clear();
    if (!canvases.isEmpty()) detectDisplay(canvases.first());
    dialog.exec();
    for (auto* canvas : canvases) QObject::disconnect(canvas, nullptr, &dialog, nullptr);
    QObject::disconnect(engine, nullptr, &dialog, nullptr);
    QMetaObject::invokeMethod(
        engine, [engine, session] { engine->comparisonStop(session); }, Qt::QueuedConnection);
}
} // namespace aigle
