#include "command_json.hpp"
#include "reply_json.hpp"
#include "export_json.hpp"
#include "editor_state_json.hpp"
#include "mask_json.hpp"
#include "retouch_json.hpp"
#include "library_json.hpp"
#include "agent_json.hpp"
#include "commands.hpp"
#include "gui_json.hpp"
#include "selection_metadata.hpp"
#include "localization.hpp"
#include "photo_identity.hpp"
#include "window.hpp"
#include <QScreen>
#include "scopes.hpp"
#include "comparison.hpp"
#include "library_query.hpp"
#include <QFile>
#include <QSet>
#include "adjustments.hpp"
#include "agent_panel.hpp"
#include "canvas.hpp"
#include "export_dialog.hpp"
#include "photo_delegate.hpp"
#include "theme.hpp"
#include "tool_panel.hpp"
#include "retouch_panel.hpp"
#include "value_editor.hpp"
#include <QActionGroup>
#include <QApplication>
#include <QButtonGroup>
#include <QCheckBox>
#include <QClipboard>
#include <QCloseEvent>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QDir>
#include <QDoubleSpinBox>
#include <QDragEnterEvent>
#include <QFileDialog>
#include <QFileInfo>
#include <QFormLayout>
#include <QInputDialog>
#include <QJsonDocument>
#include <QMenuBar>
#include <QMessageBox>
#include <QMimeData>
#include <QMouseEvent>
#include <QPainter>
#include <QPainterPath>
#include <QPushButton>
#include <QResizeEvent>
#include <QSaveFile>
#include <QScopeGuard>
#include <QScrollArea>
#include <QSignalBlocker>
#include <QSlider>
#include <QStackedWidget>
#include <QStandardPaths>
#include <QStatusBar>
#include <QStyle>
#include <QTabWidget>
#include <QToolButton>
#include <QVBoxLayout>
#include <QWindow>
#include <algorithm>
#include <cmath>

namespace aigle {

Window::Window(QVulkanInstance* instance, QString initial, bool synthetic, bool smoke,
               std::optional<EngineCommand> initialCommand)
    : engine_(new Engine), canvas_(new Canvas(instance)), adjustments_(new Adjustments),
      histogram_(new Scopes), agent_(new AgentPanel), files_(new QListWidget),
      history_(new QListWidget), status_(new QLabel), display_(new QLabel), info_(new QLabel),
      tags_(new QLineEdit), filter_(new QLineEdit), rating_(new QSpinBox), smoke_(smoke) {
    exportQueue_ = ExportQueue(QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation) + "/aigle/qt-export-queue.json");
    exportQueue_.load();
    preferences_ = Preferences::load(settings_);
    buildWorkspace();
    buildMenus();
    initializeShortcuts();
    setHdr(preferences_.hdr);
    restoreGeometry(settings_.value("gpuiLayout/geometry").toByteArray());
    setMode(false);
    libraryViewTimer_.setSingleShot(true);
    libraryViewTimer_.setInterval(150);
    connect(&libraryViewTimer_, &QTimer::timeout, this, &Window::sortPhotos);
    adjustments_->setEnabled(false);
    viewportTimer_.setSingleShot(true);
    viewportTimer_.setInterval(16);
    connect(&viewportTimer_, &QTimer::timeout, this, [this] {
        if (open_ && pendingViewport_)
            emit viewportRequested(*pendingViewport_);
    });
    autosave_.setSingleShot(true);
    autosave_.setInterval(preferences_.autosaveMs);
    connect(&autosave_, &QTimer::timeout, this, [this] {
        if (open_ && !smoke_)
            emit request(commands::Save{});
    });
    connect(this, &Window::viewportRequested, engine_, &Engine::setViewport, Qt::QueuedConnection);
    connect(canvas_, &Canvas::viewportChanged, this, [this](const ViewportRequest& request) {
        pendingViewport_ = request;
        if (!viewportTimer_.isActive())
            viewportTimer_.start();
    });
    connect(canvas_, &Canvas::displayStatus, display_, &QLabel::setText);
    connect(canvas_, &QWindow::screenChanged, this, [this] { detectDisplay(); });
    connect(canvas_, &Canvas::toolCommand, this, [this](EngineCommand command) {
        if (auto* tool = std::get_if<commands::Tool>(&command); tool && tool->tool == "color_pick") tool->mask_id = toolsPanel_->selectedMaskId();
        emit request(command);
    });
    connect(adjustments_, &Adjustments::command, this, &Window::request);
    connect(files_, &QListWidget::itemClicked, this, [this](QListWidgetItem* item) {
        const auto path = item->data(Qt::UserRole).toString();
        if (path != path_ || item->data(PhotoDelegate::CopyRole).toLongLong() != (state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null)).toInteger() || !open_)
            open(path, developMode_, item->data(PhotoDelegate::CopyRole).toLongLong());
    });
    connect(files_, &QListWidget::itemActivated, this,
            [this](QListWidgetItem* item) { open(item->data(Qt::UserRole).toString(), true, item->data(PhotoDelegate::CopyRole).toLongLong()); });
    connect(adjustments_, &Adjustments::toolSelected, this, &Window::selectTool);
    connect(filter_, &QLineEdit::textChanged, this, [this](const QString& text) {
        Q_UNUSED(text);
        filterPhotos();
    });
    connect(history_, &QListWidget::itemActivated, this, [this](QListWidgetItem* item) {
        emit request(commands::Goto{.position = history_->row(item)});
    });
    connect(rating_, qOverload<int>(&QSpinBox::valueChanged), this, [this](int rating) {
        if (open_)
            emit request(commands::Metadata{.rating = rating});
    });
    connect(tags_, &QLineEdit::editingFinished, this, [this] {
        if (!open_)
            return;
        QStringList tags;
        for (const auto& tag : tags_->text().split(',', Qt::SkipEmptyParts))
            tags.append(tag.trimmed());
        emit request(commands::Metadata{.tags = tags});
    });

    engine_->moveToThread(&thread_);
    connect(&thread_, &QThread::finished, engine_, &QObject::deleteLater);
    connect(this, &Window::request, engine_, &Engine::command, Qt::QueuedConnection);
    connect(engine_, &Engine::reply, this, &Window::receive);
    connect(engine_, &Engine::stateChanged, this, &Window::updateState);
    connect(engine_, &Engine::frameReady, canvas_, &Canvas::setFrame);
    connect(engine_, &Engine::frameReady, agent_, [this](const Frame& frame, bool original) {
        if (original || !state_.agent.proposal)
            return;
        const auto pixels = aigle_frame_pixels(frame.get());
        if (pixels.format == 0)
            agent_->setPreview(QImage(pixels.data, int(pixels.width), int(pixels.height),
                                      pixels.stride, QImage::Format_ARGB32));
    });
    connect(engine_, &Engine::thumbnailReady, this,
            [this](const QString& path, const QImage& image) {
                for (int i = 0; i < files_->count(); ++i)
                    if (files_->item(i)->data(Qt::UserRole).toString() == path) {
                        {
                            files_->item(i)->setIcon(QPixmap::fromImage(image));
                            files_->item(i)->setData(PhotoDelegate::ImageRole, image);
                        }
                    }
            });
    connect(engine_, &Engine::error, this, &Window::showError);
    connect(engine_, &Engine::ready, this,
            [this, initial = std::move(initial), synthetic,
             initialCommand = std::move(initialCommand)] {
                engineReady_ = true;
                detectDisplay();
                checkExportQueue();
                emit request(commands::Schema{});
                if (initialCommand.has_value())
                    emit request(*initialCommand);
                else if (synthetic || (smoke_ && initial.isEmpty())) {
                    pendingPath_ = tr("Color chart");
                    emit request(commands::Synthetic{});
                } else if (!initial.isEmpty()) {
                    if (QFileInfo(initial).isDir())
                        openFolder(initial);
                    else
                        open(initial);
                } else
                    status_->setText(tr("Open a photo or folder to begin"));
            });
    thread_.start();
    const EngineConfig config{.dataDirectory = QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation) + "/aigle"};
    QMetaObject::invokeMethod(
        engine_, [engine = engine_, config] { engine->start(config); }, Qt::QueuedConnection);
    if (smoke_) {
        connect(canvas_, &Canvas::captured, this, [this](const QImage& image) {
            auto screenshot = grab().toImage();
            QPainter painter(&screenshot);
            const auto position = canvas_->mapToGlobal(QPoint(0, 0)) - mapToGlobal(QPoint(0, 0));
            painter.drawImage(QRect(position, canvas_->size()), image);
            painter.end();
            screenshot.save(qEnvironmentVariable("AIGLE_SMOKE_CAPTURE") + "-window.png");
            emit smokeFinished(0);
        });
        connect(canvas_, &Canvas::rendered, this, [this](bool hdr, quint64 revision) {
            if (smokeStep_ == 0) {
                smokeStep_ = 1;
                emit request(
                    commands::Edit{.label = "Exposure", .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::Set, "light.exposure", 1.0}}});
            } else if (smokeStep_ == 4 &&
                       revision == state_.revision) {
                smokeStep_ = 5;
                qInfo() << "Qt smoke: edit, undo, redo and rendering passed; HDR active:" << hdr;
                const auto capture = qEnvironmentVariable("AIGLE_SMOKE_CAPTURE");
                if (!capture.isEmpty() && qEnvironmentVariable("AIGLE_SMOKE_LAYOUT") == "library") {
                    setMode(false);
                    QTimer::singleShot(500, this, [this, capture] {
                        grab().save(capture + "-window.png");
                        emit smokeFinished(0);
                    });
                } else if (!capture.isEmpty() && !hdr)
                    canvas_->capture(capture);
                else
                    emit smokeFinished(0);
            }
        });
        QTimer::singleShot(60000, this, [this] {
            if (smokeStep_ != 5)
                emit smokeFinished(3);
        });
    }
}
bool Window::eventFilter(QObject* watched, QEvent* event) {
    if (shortcutOverride(watched, event)) return true;
    if (watched == files_->viewport() && event->type() == QEvent::Resize)
        QTimer::singleShot(0, this, &Window::updatePhotoLayout);
    if (event->type() == QEvent::MouseButtonPress && !isMaximized()) {
        auto* widget = qobject_cast<QWidget*>(watched);
        auto* mouse = static_cast<QMouseEvent*>(event);
        if (widget && widget->window() == this && mouse->button() == Qt::LeftButton) {
            const auto point = mapFromGlobal(mouse->globalPosition().toPoint());
            Qt::Edges edges;
            if (point.x() < 5)
                edges |= Qt::LeftEdge;
            if (point.x() >= width() - 5)
                edges |= Qt::RightEdge;
            if (point.y() < 5)
                edges |= Qt::TopEdge;
            if (point.y() >= height() - 5)
                edges |= Qt::BottomEdge;
            if (edges && windowHandle() && windowHandle()->startSystemResize(edges))
                return true;
        }
    }
    return QMainWindow::eventFilter(watched, event);
}
void Window::setHdr(bool enabled) {
    hdr_->setChecked(enabled);
}
Window::~Window() {
    qApp->setProperty("aigleEffectiveShortcuts", QVariant{});
    QMetaObject::invokeMethod(engine_, &Engine::stop, Qt::BlockingQueuedConnection);
    thread_.quit();
    thread_.wait();
}
namespace {
class WorkspaceHeader final : public QWidget {
  public:
    using QWidget::QWidget;

  protected:
    void mousePressEvent(QMouseEvent* event) override {
        if (event->button() == Qt::LeftButton && window()->windowHandle())
            window()->windowHandle()->startSystemMove();
    }
    void mouseDoubleClickEvent(QMouseEvent* event) override {
        if (event->button() == Qt::LeftButton)
            window()->isMaximized() ? window()->showNormal() : window()->showMaximized();
    }
};
QVBoxLayout* column(QWidget* widget, int padding = 0) {
    auto* layout = new QVBoxLayout(widget);
    layout->setContentsMargins(padding, padding, padding, padding);
    layout->setSpacing(8);
    return layout;
}
} // namespace
void Window::buildWorkspace() {
    setObjectName("aigle");
    setWindowTitle("Aigle");
    setWindowFlag(Qt::FramelessWindowHint);
    resize(1440, 900);
    setMinimumSize(1024, 700);
    setAcceptDrops(true);
    auto* shell = new QWidget;
    auto* shellLayout = column(shell);
    shellLayout->setSpacing(0);
    setCentralWidget(shell);
    auto* titleBar = new WorkspaceHeader;
    titleBar->setObjectName("titleBar");
    titleBar->setFixedHeight(28);
    auto* titleRow = new QHBoxLayout(titleBar);
    titleRow->setContentsMargins(13, 0, 4, 0);
    auto* brand = new QLabel("Aigle");
    brand->setObjectName("muted");
    brand->setFixedWidth(110);
    titleRow->addWidget(brand);
    titleRow->addStretch();
    title_ = new QLabel("Aigle");
    title_->setObjectName("headerTitle");
    titleRow->addWidget(title_);
    titleRow->addStretch();
    for (const auto& text : {QString("−"), QString("□"), QString("×")}) {
        auto* b = new QToolButton;
        b->setText(text);
        b->setFixedSize(28, 24);
        b->setObjectName(text == "×" ? "windowClose" : "windowControl");
        b->setAccessibleName(text == "×"   ? tr("Close")
                             : text == "−" ? tr("Minimize")
                                           : tr("Maximize"));
        titleRow->addWidget(b);
        connect(b, &QToolButton::clicked, this, [this, text] {
            if (text == "×")
                close();
            else if (text == "−")
                showMinimized();
            else
                isMaximized() ? showNormal() : showMaximized();
        });
    }
    shellLayout->addWidget(titleBar);
    appMenus_ = new QMenuBar;
    appMenus_->setNativeMenuBar(false);
    appMenus_->setFixedHeight(24);
    shellLayout->addWidget(appMenus_);
    auto* header = new QWidget;
    header->setObjectName("appHeader");
    header->setFixedHeight(44);
    auto* bar = new QHBoxLayout(header);
    bar->setContentsMargins(12, 0, 12, 0);
    bar->setSpacing(14);
    gridButton_ = new QToolButton;
    developButton_ = new QToolButton;
    for (auto* b : {gridButton_, developButton_}) {
        b->setObjectName("workspaceTab");
        b->setToolButtonStyle(Qt::ToolButtonTextOnly);
        b->setFixedHeight(44);
        bar->addWidget(b);
    }
    breadcrumb_ = new QLabel;
    breadcrumb_->setObjectName("muted");
    breadcrumb_->setMinimumWidth(0);
    breadcrumb_->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    bar->addWidget(breadcrumb_, 1);
    queueButton_ = new QToolButton;
    bindTranslation(queueButton_, [this, target = queueButton_] { target->setText(tr("Jobs")); });
    queueButton_->setPopupMode(QToolButton::InstantPopup);
    bar->addWidget(queueButton_);
    exportButton_ = new QToolButton;
    bindTranslation(exportButton_, [this, target = exportButton_] { target->setText(tr("Export…")); });
    exportButton_->setObjectName("primary");
    exportButton_->setFixedSize(95, 28);
    bar->addWidget(exportButton_);
    connect(exportButton_, &QToolButton::clicked, this, &Window::exportPhoto);
    shellLayout->addWidget(header);
    workspace_ = new QStackedWidget;
    shellLayout->addWidget(workspace_, 1);
    auto* statusRow = new QWidget;
    statusRow->setObjectName("statusRow");
    statusRow->setFixedHeight(22);
    auto* statusLayout = new QHBoxLayout(statusRow);
    statusLayout->setContentsMargins(12, 0, 12, 0);
    status_->setObjectName("statusText");
    status_->setText(tr("Open a photo or folder to begin"));
    statusLayout->addWidget(status_, 1);
    selection_ = new QLabel;
    selection_->setObjectName("statusText");
    statusLayout->addWidget(selection_, 1, Qt::AlignCenter);
    display_->setObjectName("statusText");
    statusLayout->addWidget(display_, 1, Qt::AlignRight);
    shellLayout->addWidget(statusRow);
    statusBar()->hide();
    connect(statusBar(), &QStatusBar::messageChanged, status_, &QLabel::setText);

    // Library: sources | grid | metadata, with selection actions below the grid.
    auto* library = new QWidget;
    auto* libraryRow = new QHBoxLayout(library);
    libraryRow->setContentsMargins(0, 0, 0, 0);
    libraryRow->setSpacing(0);
    sources_ = new QWidget;
    sources_->setObjectName("librarySources");
    sources_->setFixedWidth(208);
    auto* sourceLayout = column(sources_);
    sourceLayout->setSpacing(0);
    const auto sourceHeading = [sourceLayout](const char* text) {
        auto* l = new QLabel;
        bindTranslation(l, [l, text] { l->setText(Window::tr(text)); });
        l->setObjectName("sourceHeading");
        l->setContentsMargins(14, 0, 0, 0);
        sourceLayout->addWidget(l);
    };
    const auto sourceButton = [sourceLayout](const char* text, const QString& icon) {
        auto* b = new QToolButton;
        b->setObjectName("sourceItem");
        bindTranslation(b, [b, text] { b->setText(Window::tr(text)); });
        b->setIcon(glyph(icon));
        b->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
        b->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
        b->setFixedHeight(34);
        sourceLayout->addWidget(b);
        return b;
    };
    sourceHeading(QT_TR_NOOP("Sources"));
    auto* openFolderButton = sourceButton(QT_TR_NOOP("Open folder…"), "folder");
    connect(openFolderButton, &QToolButton::clicked, this, [this] {
        const auto p = QFileDialog::getExistingDirectory(this, tr("Open folder"), folder_);
        if (!p.isEmpty())
            openFolder(p);
    });
    sourceHeading(QT_TR_NOOP("Folders"));
    folderTitle_ = new QLabel(tr("No folder open"));
    folderTitle_->setWordWrap(true);
    folderTitle_->setObjectName("folderTitle");
    folderTitle_->setContentsMargins(28, 10, 12, 10);
    sourceLayout->addWidget(folderTitle_);
    sourceHeading(QT_TR_NOOP("Albums"));
    auto* albums = sourceButton(QT_TR_NOOP("Open album…"), "folder");
    connect(albums, &QToolButton::clicked, this, [this] { emit request(commands::Albums{}); });
    sourceHeading(QT_TR_NOOP("Saved searches"));
    auto* searches = sourceButton(QT_TR_NOOP("Manage saved searches…"), "folder");
    connect(searches, &QToolButton::clicked, this, &Window::manageSavedSearches);
    sourceLayout->addStretch();
    libraryRow->addWidget(sources_);
    auto* libraryMain = new QWidget;
    auto* libraryMainLayout = column(libraryMain);
    libraryMainLayout->setSpacing(0);
    libraryRow->addWidget(libraryMain, 1);
    auto* libraryTop = new QWidget;
    auto* libraryTopLayout = new QHBoxLayout(libraryTop);
    libraryTopLayout->setContentsMargins(0, 0, 0, 0);
    libraryTopLayout->setSpacing(0);
    libraryMainLayout->addWidget(libraryTop, 1);
    auto* gridColumn = new QWidget;
    auto* gridColumnLayout = column(gridColumn);
    gridColumnLayout->setSpacing(0);
    libraryTopLayout->addWidget(gridColumn, 1);
    auto* filters = new QWidget;
    filters->setObjectName("libraryFilters");
    filters->setFixedHeight(58);
    auto* filterLayout = new QHBoxLayout(filters);
    filterLayout->setContentsMargins(12, 10, 18, 10);
    filterLayout->setSpacing(8);
    auto* sourceToggle = iconButton("grid", tr("Show or hide sources"));
    filterLayout->addWidget(sourceToggle);
    connect(sourceToggle, &QToolButton::clicked, this, [this] {
        leftOverride_ = sources_->isVisible() ? 0 : 1;
        updateResponsiveLayout();
    });
    filter_->setObjectName("librarySearch");
    bindTranslation(filter_, [this, target = filter_] { target->setPlaceholderText(tr("Search photos and metadata…")); });
    bindTranslation(filter_, [this, target = filter_] { target->setToolTip(tr("All words must match. Prefix a word with - to exclude it, e.g. beach -portrait.")); });
    filter_->setFixedHeight(32);
    filter_->setMaximumWidth(280);
    filterLayout->addWidget(filter_, 1);
    auto* filterButton = new QToolButton;
    filterButton->setObjectName("libraryRatingFilter");
    bindTranslation(filterButton, [this, target = filterButton] { target->setText(tr("Filter")); });
    filterButton->setIcon(glyph("filter"));
    filterButton->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    bindTranslation(filterButton, [this, target = filterButton] { target->setToolTip(tr("Filter by rating, flag, label, camera, lens, date, or keyword")); });
    filterLayout->addWidget(filterButton);
    connect(filterButton, &QToolButton::clicked, this, &Window::editLibraryFilters);
    filterLayout->addStretch();
    auto* sort = new QComboBox;
    sort->setObjectName("librarySort");
    sort->addItems({tr("File name"), tr("Rating"), tr("Capture date"), tr("Modified date")});
    bindTranslation(sort, [this, sort] {
        const QSignalBlocker blocker(sort);
        const QStringList labels{tr("File name"), tr("Rating"), tr("Capture date"), tr("Modified date")};
        for (int i = 0; i < labels.size(); ++i) sort->setItemText(i, labels[i]);
    });
    sort->setFixedWidth(144);
    filterLayout->addWidget(sort);
    connect(sort, qOverload<int>(&QComboBox::currentIndexChanged), this, &Window::sortPhotos);
    auto* descending = new QToolButton;
    descending->setObjectName("libraryDescending");
    bindTranslation(descending, [this, target = descending] { target->setText(tr("↓")); });
    descending->setCheckable(true);
    bindTranslation(descending, [this, target = descending] { target->setToolTip(tr("Descending sort order")); });
    filterLayout->addWidget(descending);
    connect(descending, &QToolButton::toggled, this, &Window::sortPhotos);
    gridColumnLayout->addWidget(filters);
    auto* gridHeading = new QWidget;
    auto* gh = new QHBoxLayout(gridHeading);
    gh->setContentsMargins(20, 12, 20, 8);
    auto* gridLabel = new QLabel(tr("Photographs"));
    bindTranslation(gridLabel, [this, label = gridLabel] { label->setText(tr("Photographs")); });
    gh->addWidget(gridLabel);
    gridCount_ = new QLabel;
    gridCount_->setObjectName("muted");
    gh->addWidget(gridCount_, 1, Qt::AlignRight);
    gridColumnLayout->addWidget(gridHeading);
    gridHolder_ = new QWidget;
    gridHolder_->setObjectName("gridHolder");
    auto* gridLayout = column(gridHolder_);
    gridLayout->setContentsMargins(14, 0, 14, 12);
    gridLayout->setSpacing(0);
    emptyLibrary_ = new QLabel(tr("Open a photo or folder to begin"));
    emptyLibrary_->setAlignment(Qt::AlignCenter);
    emptyLibrary_->setObjectName("emptyHint");
    gridLayout->addWidget(emptyLibrary_, 1);
    gridColumnLayout->addWidget(gridHolder_, 1);
    libraryMetadata_ = new QWidget;
    libraryMetadata_->setObjectName("libraryMetadata");
    libraryMetadata_->setFixedWidth(312);
    auto* ml = column(libraryMetadata_);
    ml->setSpacing(0);
    auto* mt = new QLabel(tr("Metadata"));
    bindTranslation(mt, [this, label = mt] { label->setText(tr("Metadata")); });
    mt->setFixedHeight(39);
    mt->setContentsMargins(18, 0, 0, 0);
    ml->addWidget(mt);
    auto* metaBody = new QWidget;
    auto* metadataLayout = column(metaBody, 16);
    metadataLayout->setSpacing(12);
    librarySelection_ = new QLabel(tr("Select a photo"));
    librarySelection_->setObjectName("sectionTitle");
    metadataLayout->addWidget(librarySelection_);
    libraryPreview_ = new QLabel;
    libraryPreview_->setObjectName("photoPreview");
    libraryPreview_->setAlignment(Qt::AlignCenter);
    libraryPreview_->setFixedHeight(160);
    metadataLayout->addWidget(libraryPreview_);
    libraryInfo_ = new QLabel;
    libraryInfo_->setWordWrap(true);
    libraryInfo_->setObjectName("muted");
    metadataLayout->addWidget(libraryInfo_);
    libraryRating_ = new QSpinBox;
    libraryRating_->setRange(0, 5);
    libraryRating_->setSpecialValueText(tr("Unrated"));
    selectionMetadata_ = new SelectionMetadata;
    selectionMetadata_->command = [this](const EngineCommand& command) { emit request(command); };
    auto* stars = new QWidget;
    stars->setObjectName("ratingStars");
    auto* starLayout = new QHBoxLayout(stars);
    starLayout->setContentsMargins(0, 0, 0, 0);
    starLayout->setSpacing(2);
    auto* starsLabel = new QLabel;
    bindTranslation(starsLabel, [this, starsLabel] { starsLabel->setText(tr("Rating")); });
    starLayout->addWidget(starsLabel, 1);
    libraryRating_->setParent(metaBody);
    libraryRating_->hide();
    for (int rating = 1; rating <= 5; ++rating) {
        auto* star = new QToolButton;
        star->setText(QString::fromUtf8("☆"));
        star->setStyleSheet("font-size:18px;padding:0");
        star->setFixedSize(22, 26);
        star->setAccessibleName(tr("Rate %1 stars").arg(rating));
        star->setToolTip(tr("Rate %1 stars; click again to clear").arg(rating));
        starLayout->addWidget(star);
        connect(star, &QToolButton::clicked, this, [this, rating] {
            if (files_->selectedItems().size() > 1)
                batchMetadata(commands::Metadata{.rating = rating});
            else
                libraryRating_->setValue(libraryRating_->value() == rating ? 0 : rating);
        });
        connect(libraryRating_, qOverload<int>(&QSpinBox::valueChanged), star,
                [star, rating](int value) {
                    star->setText(value >= rating ? QString::fromUtf8("★")
                                                  : QString::fromUtf8("☆"));
                });
    }
    metadataLayout->addWidget(stars);
    auto* batchActions = new QPushButton(tr("Set selection metadata…"));
    bindTranslation(batchActions, [this, button = batchActions] { button->setText(tr("Set selection metadata…")); });
    batchActions->setObjectName("batchMetadataActions");
    metadataLayout->addWidget(batchActions);
    connect(batchActions, &QPushButton::clicked, this, [this] {
        const QStringList actions{tr("Clear rating"), tr("Pick"),       tr("Reject"),
                                  tr("Clear flag"),   tr("Red label"),  tr("Yellow label"),
                                  tr("Green label"),  tr("Blue label"), tr("Purple label"),
                                  tr("Clear label"), tr("Add keywords…"), tr("Remove keywords…"),
                                  tr("Replace keywords…"), tr("Set descriptive field…")};
        bool ok = false;
        const auto chosen = QInputDialog::getItem(
            this, tr("Selection metadata"),
            tr("Apply to %1 selected photos").arg(files_->selectedItems().size()), actions, 0,
            false, &ok);
        if (!ok)
            return;
        const auto index = actions.indexOf(chosen);
        if (index < 0)
            return;
        if (index >= 10 && index <= 12) {
            const auto text = QInputDialog::getText(
                this, chosen,
                index == 11 ? tr("Comma-separated whole keywords or hierarchy paths to remove")
                            : index == 12 ? tr("Replace all keywords (leave empty to clear)")
                                          : tr("Comma-separated keywords to add"),
                QLineEdit::Normal, {}, &ok);
            if (!ok)
                return;
            QStringList tags;
            for (const auto& tag : text.split(','))
                if (!tag.trimmed().isEmpty())
                    tags.append(tag.trimmed());
            if (tags.isEmpty() && index != 12)
                return;
            batchMetadata(commands::Metadata{.tags = tags, .tag_mode = QStringList{"add", "remove", "replace"}[index - 10]});
        } else if (index == 13) {
            const auto fields = metadataSchema_["properties"].toObject().keys();
            if (fields.isEmpty())
                return;
            const auto field = QInputDialog::getItem(this, chosen, tr("Field to change"),
                                                     fields, 0, false, &ok);
            if (!ok)
                return;
            const auto text = QInputDialog::getMultiLineText(
                this, chosen, tr("New value for %1 (leave empty to clear)").arg(field), {}, &ok);
            if (ok)
                batchMetadata(commands::Metadata{.iptc = QMap<QString, QString>{{field, text}}});
        } else if (index == 0)
            batchMetadata(commands::Metadata{.rating = 0});
        else if (index <= 3)
            batchMetadata(commands::Metadata{.flag = QStringList{"picked", "rejected", "none"}[index - 1]});
        else if (index == 9)
            batchMetadata(commands::Metadata{.label = QJsonValue::Null});
        else
            batchMetadata(
                commands::Metadata{.label = QStringList{"red", "yellow", "green", "blue", "purple"}[index - 4]});
    });
    metadataLayout->addWidget(selectionMetadata_);
    metadataLayout->addStretch();
    auto* metaScroll = new QScrollArea;
    metaScroll->setWidgetResizable(true);
    metaScroll->setWidget(metaBody);
    ml->addWidget(metaScroll, 1);
    libraryTopLayout->addWidget(libraryMetadata_);
    connect(libraryRating_, qOverload<int>(&QSpinBox::valueChanged), rating_, &QSpinBox::setValue);
    auto* libraryBottom = new QWidget;
    libraryBottom->setObjectName("inspectorFooter");
    libraryBottom->setFixedHeight(42);
    auto* lb = new QHBoxLayout(libraryBottom);
    lb->setContentsMargins(12, 0, 16, 0);
    auto* importButton = new QPushButton(glyph("import"), tr("Import…"));
    bindTranslation(importButton, [this, button = importButton] { button->setText(tr("Import…")); });
    lb->addWidget(importButton);
    connect(importButton, &QPushButton::clicked, this, [this] {
        for (auto* a : findChildren<QAction*>())
            if (a->text() == tr("Import photos…")) {
                a->trigger();
                break;
            }
    });
    lb->addStretch();
    auto* thumbLabel = new QLabel(tr("Thumbnail size"));
    bindTranslation(thumbLabel, [this, label = thumbLabel] { label->setText(tr("Thumbnail size")); });
    thumbLabel->setObjectName("muted");
    lb->addWidget(thumbLabel);
    auto* size = new QSlider(Qt::Horizontal);
    size->setRange(160, 340);
    size->setObjectName("thumbnailSize");
    size->setValue(preferences_.thumbnail);
    files_->setProperty("preferredCell", preferences_.thumbnail);
    size->setFixedWidth(90);
    lb->addWidget(size);
    connect(size, &QSlider::valueChanged, this, [this](int value) {
        files_->setProperty("preferredCell", value);
        preferences_.thumbnail = value;
        settings_.setValue("preferences/thumbnail", value);
        updatePhotoLayout();
    });
    libraryMainLayout->addWidget(libraryBottom);
    workspace_->addWidget(library);

    // Develop: the left rail and canvas share a filmstrip; the inspector is full height.
    auto* develop = new QWidget;
    auto* developRow = new QHBoxLayout(develop);
    developRow->setContentsMargins(0, 0, 0, 0);
    developRow->setSpacing(0);
    auto* imageWorkspace = new QWidget;
    auto* imageLayout = column(imageWorkspace);
    imageLayout->setSpacing(0);
    developRow->addWidget(imageWorkspace, 1);
    developContent_ = new QStackedWidget;
    auto* empty = new QLabel(tr("Open a photo to start editing"));
    bindTranslation(empty, [this, label = empty] { label->setText(tr("Open a photo to start editing")); });
    empty->setAlignment(Qt::AlignCenter);
    empty->setObjectName("emptyHint");
    developContent_->addWidget(empty);
    auto* editor = new QWidget;
    auto* editorRow = new QHBoxLayout(editor);
    editorRow->setContentsMargins(0, 0, 0, 0);
    editorRow->setSpacing(0);
    leftRail_ = new QWidget;
    leftRail_->setObjectName("leftSidebar");
    leftRail_->setFixedWidth(208);
    auto* leftLayout = column(leftRail_);
    leftLayout->setSpacing(0);
    leftLayout->addWidget(histogram_);
    connect(histogram_, &Scopes::command, this, &Window::request);
    references_ = new QTabWidget;
    references_->setObjectName("referenceTabs");
    references_->setDocumentMode(true);
    history_->setObjectName("history");
    references_->addTab(history_, tr("History"));
    snapshots_ = new QListWidget;
    auto* snapshotPage = new QWidget;
    auto* sl = column(snapshotPage);
    sl->addWidget(snapshots_, 1);
    auto* newSnapshot = new QPushButton(tr("+ Create snapshot"));
    bindTranslation(newSnapshot, [this, button = newSnapshot] { button->setText(tr("+ Create snapshot")); });
    sl->addWidget(newSnapshot);
    references_->addTab(snapshotPage, tr("Snapshots"));
    connect(newSnapshot, &QPushButton::clicked, this, [this] {
        if (!open_)
            return;
        const auto name = QInputDialog::getText(this, tr("Create snapshot"), tr("Name"));
        if (!name.trimmed().isEmpty())
            emit request(commands::Snapshot{.name = name});
    });
    connect(snapshots_, &QListWidget::itemActivated, this, [this](QListWidgetItem* item) {
        emit request(commands::RestoreSnapshot{.index = snapshots_->row(item)});
    });
    presets_ = new QListWidget;
    auto* presetPage = new QWidget;
    auto* pl = column(presetPage);
    pl->addWidget(presets_, 1);
    auto* addPreset = new QPushButton(tr("+ Save preset"));
    bindTranslation(addPreset, [this, button = addPreset] { button->setText(tr("+ Save preset")); });
    pl->addWidget(addPreset);
    references_->addTab(presetPage, tr("Presets"));
    connect(addPreset, &QPushButton::clicked, this, &Window::savePreset);
    connect(presets_, &QListWidget::itemActivated, this,
            [this](QListWidgetItem* item) { applyPreset(item->data(Qt::UserRole).toString()); });
    bindTranslation(references_, [this] {
        references_->setTabText(0, tr("History"));
        references_->setTabText(1, tr("Snapshots"));
        references_->setTabText(2, tr("Presets"));
    });
    historyPane_ = references_;
    leftLayout->addWidget(references_, 1);
    editorRow->addWidget(leftRail_);
    auto* canvasColumn = new QWidget;
    auto* canvasLayout = column(canvasColumn);
    canvasLayout->setSpacing(0);
    editorRow->addWidget(canvasColumn, 1);
    auto* toolbar = new QWidget;
    toolbar->setObjectName("canvasToolbar");
    toolbar->setFixedHeight(36);
    auto* tb = new QHBoxLayout(toolbar);
    tb->setContentsMargins(8, 3, 10, 3);
    tb->setSpacing(3);
    auto* railToggle = iconButton("grid", tr("Show or hide reference rail"));
    tb->addWidget(railToggle);
    connect(railToggle, &QToolButton::clicked, this, [this] {
        leftOverride_ = leftRail_->isVisible() ? 0 : 1;
        updateResponsiveLayout();
    });
    for (const auto& entry :
         std::array<std::tuple<QString, QString, QString>, 4>{{{QT_TR_NOOP("Pan"), "pan", "view"},
                                                               {QT_TR_NOOP("Crop"), "crop", "select"},
                                                               {QT_TR_NOOP("Retouch"), "heal", "heal"},
                                                               {QT_TR_NOOP("Masks"), "masks", "brush"}}}) {
        const auto [label, id, icon] = entry;
        auto* b = new QToolButton;
        bindTranslation(b, [this, b, label] { b->setText(tr(qPrintable(label))); });
        b->setIcon(glyph(icon));
        b->setIconSize({14, 14});
        b->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
        b->setCheckable(true);
        b->setChecked(id == "pan");
        b->setProperty("tool", id);
        b->setFixedHeight(26);
        canvasTools_.append(b);
        tb->addWidget(b);
        connect(b, &QToolButton::clicked, this, [this, id] { showInspector(id); });
    }
    tb->addStretch();
    compareButton_ = new QToolButton;
    bindTranslation(compareButton_, [this, target = compareButton_] { target->setText(tr("Before / after")); });
    compareButton_->setCheckable(true);
    tb->addWidget(compareButton_);
    zoomButton_ = new QToolButton;
    zoomButton_->setText(tr("Fit ⌄"));
    zoomButton_->setPopupMode(QToolButton::InstantPopup);
    auto* zoomMenu = new QMenu(zoomButton_);
    zoomMenu->addAction(tr("Fit"), canvas_, &Canvas::fit);
    zoomMenu->addAction(tr("100%"), canvas_, &Canvas::oneToOne);
    zoomButton_->setMenu(zoomMenu);
    tb->addWidget(zoomButton_);
    canvasLayout->addWidget(toolbar);
    canvasSurround_ = new QWidget;
    canvasSurround_->setObjectName("canvasSurround");
    auto* surround = column(canvasSurround_, 22);
    surround->setSpacing(0);
    auto* container = QWidget::createWindowContainer(canvas_);
    container->setMinimumSize(200, 160);
    surround->addWidget(container, 1);
    canvasLayout->addWidget(canvasSurround_, 1);
    auto* cf = new QWidget;
    cf->setObjectName("canvasFooter");
    cf->setFixedHeight(28);
    auto* cfl = new QHBoxLayout(cf);
    cfl->setContentsMargins(12, 0, 12, 0);
    canvasInfo_ = new QLabel;
    canvasInfo_->setObjectName("canvasInfo");
    cfl->addWidget(canvasInfo_, 1);
    auto* filmToggle = new QToolButton;
    bindTranslation(filmToggle, [this, target = filmToggle] { target->setText(tr("Filmstrip")); });
    filmToggle->setFixedHeight(22);
    cfl->addWidget(filmToggle);
    connect(filmToggle, &QToolButton::clicked, this, [this] {
        filmOverride_ = filmstrip_->isVisible() ? 0 : 1;
        updateResponsiveLayout();
    });
    dimensions_ = new QLabel;
    dimensions_->setObjectName("canvasInfo");
    cfl->addWidget(dimensions_);
    canvasLayout->addWidget(cf);
    developContent_->addWidget(editor);
    imageLayout->addWidget(developContent_, 1);
    filmstrip_ = new QWidget;
    filmstrip_->setObjectName("filmstrip");
    filmstrip_->setFixedHeight(116);
    auto* filmLayout = column(filmstrip_);
    filmLayout->setSpacing(0);
    filmLayout->setContentsMargins(8, 0, 8, 4);
    filmCount_ = new QLabel;
    filmCount_->setObjectName("statusText");
    filmCount_->setFixedHeight(25);
    filmCount_->setContentsMargins(4, 0, 0, 0);
    filmLayout->addWidget(filmCount_);
    imageLayout->addWidget(filmstrip_);
    rightRail_ = new QWidget;
    rightRail_->setObjectName("rightSidebar");
    rightRail_->setFixedWidth(312);
    auto* rightLayout = column(rightRail_);
    rightLayout->setSpacing(0);
    developRow->addWidget(rightRail_);
    auto* modes = new QWidget;
    modes->setObjectName("rightModes");
    modes->setFixedHeight(39);
    auto* modeLayout = new QHBoxLayout(modes);
    modeLayout->setContentsMargins(16, 0, 16, 0);
    modeLayout->setSpacing(18);
    auto* modeGroup = new QButtonGroup(this);
    for (int i = 0; i < 2; ++i) {
        auto* b = new QToolButton;
        b->setObjectName("workspaceTab");
        bindTranslation(b, [this, b, i] { b->setText(i == 0 ? tr("Edit") : tr("Agent")); });
        b->setProperty("inspectorMode", i);
        b->setFixedHeight(39);
        b->setCheckable(true);
        b->setChecked(i == 0);
        modeGroup->addButton(b, i);
        modeLayout->addWidget(b);
    }
    modeLayout->addStretch();
    auto* undoButton = iconButton("reset", tr("Undo"));
    modeLayout->addWidget(undoButton);
    connect(undoButton, &QToolButton::clicked, this, [this] {
        if (undo_)
            undo_->trigger();
    });
    rightLayout->addWidget(modes);
    rightPages_ = new QStackedWidget;
    rightPages_->addWidget(adjustments_);
    rightPages_->addWidget(agent_);
    toolsPanel_ = new ToolPanel;
    rightPages_->addWidget(toolsPanel_);
    retouch_=new RetouchPanel;
    rightPages_->addWidget(retouch_);
    connect(retouch_,&RetouchPanel::command,this,&Window::request);
    connect(retouch_,&RetouchPanel::selected,canvas_,&Canvas::selectRetouch);
    connect(retouch_,&RetouchPanel::defaultsChanged,canvas_,&Canvas::setRetouchDefaults);
    connect(retouch_,&RetouchPanel::done,this,[this]{showInspector("pan");});
    connect(canvas_,&Canvas::retouchSelected,retouch_,&RetouchPanel::select);
    connect(canvas_,&Canvas::retouchError,retouch_,&RetouchPanel::setError);
    canvas_->setRetouchDefaults(retouch_->defaults());
    connect(toolsPanel_, &ToolPanel::command, this, [this](const EngineCommand& command) {
        if (const auto* overlays = std::get_if<commands::Overlays>(&command)) updateOverlays(overlays->overlays.value_or(OverlayPatch{}));
        else emit request(command);
    });
    connect(toolsPanel_, &ToolPanel::toolSelected, this, &Window::selectTool);
    connect(toolsPanel_, &ToolPanel::done, this, [this] { showInspector("pan"); });
    connect(toolsPanel_, &ToolPanel::cropOrientationChanged, canvas_, &Canvas::orientCrop);
    connect(toolsPanel_, &ToolPanel::horizonRequested, canvas_, &Canvas::drawHorizon);
    connect(toolsPanel_, &ToolPanel::cropGuideChanged, canvas_, &Canvas::setCropGuide);
    connect(canvas_, &Canvas::cropAngleUpdated, toolsPanel_, &ToolPanel::setCropAngle);
    connect(toolsPanel_, &ToolPanel::cropAccepted, canvas_, &Canvas::applyCrop);
    connect(toolsPanel_, &ToolPanel::cropCancelled, canvas_, &Canvas::cancelCrop);
    connect(toolsPanel_, &ToolPanel::aspectChanged, canvas_, &Canvas::setCropAspect);
    connect(toolsPanel_, &ToolPanel::cropAngleChanged, canvas_, &Canvas::setCropAngle);
    connect(toolsPanel_, &ToolPanel::cropAngleGestureStarted, canvas_, &Canvas::beginCropAngleGesture);
    connect(toolsPanel_, &ToolPanel::cropAngleGestureFinished, canvas_, &Canvas::endCropAngleGesture);
    connect(canvas_, &Canvas::toolFinished, this, [this] { showInspector("pan"); });
    connect(toolsPanel_, &ToolPanel::maskSelected, canvas_, &Canvas::setMaskGuide);
    rightLayout->addWidget(rightPages_, 1);
    connect(modeGroup, &QButtonGroup::idClicked, this,
            [this](int i) { showInspector(i == 1 ? "agent" : "pan"); });
    connect(agent_, &AgentPanel::command, this, &Window::request);
    auto* footer = new QWidget;
    footer->setObjectName("inspectorFooter");
    footer->setFixedHeight(42);
    auto* fl = new QHBoxLayout(footer);
    fl->setContentsMargins(14, 0, 14, 0);
    auto* infoButton = new QToolButton;
    infoButton->setIcon(glyph("camera"));
    infoButton->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    bindTranslation(infoButton, [this, target = infoButton] { target->setText(tr("Info && metadata")); });
    fl->addWidget(infoButton);
    fl->addStretch();
    auto* reset = new QToolButton;
    bindTranslation(reset, [this, target = reset] { target->setText(tr("Reset")); });
    fl->addWidget(reset);
    rightLayout->addWidget(footer);
    connect(infoButton, &QToolButton::clicked, this, [this] {
        rightPages_->setCurrentIndex(0);
        adjustments_->showPage(4);
    });
    connect(reset, &QToolButton::clicked, this, [this] {
        if (open_)
            emit request(commands::Edit{.label = "Reset all", .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::Reset, "", {}}}});
    });
    auto* information = new QWidget;
    auto* il = column(information, 16);
    info_->setWordWrap(true);
    info_->setObjectName("muted");
    il->addWidget(info_);
    il->addStretch();
    auto* metadata = new QWidget;
    auto* form = new QFormLayout(metadata);
    form->setContentsMargins(16, 16, 16, 16);
    rating_->setRange(0, 5);
    bindTranslation(tags_, [this, target = tags_] { target->setPlaceholderText(tr("Comma-separated keywords")); });
    form->addRow(tr("Rating"), rating_);
    form->addRow(tr("Keywords"), tags_);
    auto* ratingLabel = qobject_cast<QLabel*>(form->labelForField(rating_));
    auto* keywordsLabel = qobject_cast<QLabel*>(form->labelForField(tags_));
    bindTranslation(metadata, [this, ratingLabel, keywordsLabel] {
        ratingLabel->setText(tr("Rating"));
        keywordsLabel->setText(tr("Keywords"));
    });
    auto* descriptive = new QPushButton(tr("Edit descriptive metadata…"));
    bindTranslation(descriptive, [this, button = descriptive] { button->setText(tr("Edit descriptive metadata…")); });
    form->addRow(descriptive);
    connect(descriptive, &QPushButton::clicked, this, [this] {
        auto v = QJsonValue(toJson(state_.iptc.value_or(QMap<QString, QString>{})));
        if (editValue(this, tr("Descriptive metadata"), metadataSchema_, v, metadataSchema_))
            emit request(commands::Metadata{.iptc = metadataFieldsFromJson(v.toObject())});
    });
    adjustments_->setAuxiliaryPages(information, metadata);
    workspace_->addWidget(develop);
    files_->setObjectName("photos");
    files_->setViewMode(QListView::IconMode);
    files_->setResizeMode(QListView::Adjust);
    files_->setMovement(QListView::Static);
    files_->setSelectionMode(QAbstractItemView::ExtendedSelection);
    files_->setSpacing(0);
    files_->setUniformItemSizes(true);
    photoDelegate_ = new PhotoDelegate(files_);
    files_->setItemDelegate(photoDelegate_);
    files_->viewport()->installEventFilter(this);
    connect(files_, &QListWidget::itemSelectionChanged, this, &Window::updateSelection);
    qApp->installEventFilter(this);
    refreshPresets();
    updateResponsiveLayout();
    bindTranslation(this, [this] { translateLibraryLabels(); });
    bindTranslation(this, [this] { translateDocumentStatus(); });
}
void Window::updateResponsiveLayout() {
    const int sizeClass = width() < 1100 ? 0 : width() < 1280 ? 1 : width() < 1700 ? 2 : 3;
    if (layoutClass_ != sizeClass) {
        leftOverride_ = filmOverride_ = -1;
        layoutClass_ = sizeClass;
    }
    const bool left = leftOverride_ < 0 ? width() >= 1280 : leftOverride_ == 1;
    leftRail_->setVisible(left);
    sources_->setVisible(left);
    leftRail_->setFixedWidth(width() >= 1700 ? 224 : 208);
    sources_->setFixedWidth(width() >= 1700 ? 224 : 208);
    rightRail_->setFixedWidth(width() < 1100 ? 300 : width() >= 1700 ? 328 : 312);
    libraryMetadata_->setFixedWidth(rightRail_->width());
    rightRail_->setVisible(!(width() < 1100 && left));
    libraryMetadata_->setVisible(!(width() < 1100 && left));
    filmstrip_->setVisible(developMode_ && files_->count() > 0 &&
                           (filmOverride_ < 0 ? width() >= 1100 : filmOverride_ == 1));
    for (auto* b : canvasTools_)
        b->setToolButtonStyle(width() < 1100 ? Qt::ToolButtonTextOnly
                                             : Qt::ToolButtonTextBesideIcon);
    if (canvasSurround_)
        canvasSurround_->layout()->setContentsMargins(
            width() < 1100 ? 16 : 22, width() < 1100 ? 16 : 22, width() < 1100 ? 16 : 22,
            width() < 1100 ? 16 : 22);
    QTimer::singleShot(0, this, &Window::updatePhotoLayout);
}
void Window::resizeEvent(QResizeEvent* event) {
    QMainWindow::resizeEvent(event);
    if (leftRail_)
        updateResponsiveLayout();
}
void Window::updatePhotoLayout() {
    if (!photoDelegate_)
        return;
    photoDelegate_->film = developMode_;
    files_->setProperty("filmstrip", developMode_);
    files_->style()->unpolish(files_);
    files_->style()->polish(files_);
    if (developMode_)
        photoDelegate_->cell = {114, 84};
    else {
        const int available = files_->viewport()->width();
        const int preferred = files_->property("preferredCell").toInt();
        const int cols =
            std::max(1, qRound(double(available) / std::max(180, preferred ? preferred : 285)));
        const int cell = std::max(150, available / cols - 2);
        photoDelegate_->cell = {cell, cell < 250 ? 195 : 224};
    }
    files_->setGridSize(photoDelegate_->cell);
    files_->doItemsLayout();
    files_->viewport()->update();
}
void Window::batchMetadata(commands::Metadata fields) {
    QList<PhotoIdentity> paths;
    for (auto* item : files_->selectedItems())
        paths.append(photoIdentity(item));
    if (paths.isEmpty())
        return;
    commands::BatchMetadata command;
    command.targets = paths;
    command.rating = fields.rating;
    command.flag = fields.flag;
    command.label = fields.label;
    command.tags = fields.tags;
    command.remove_tags = fields.remove_tags;
    command.tag_mode = fields.tag_mode;
    command.iptc = fields.iptc;
    command.stack = fields.stack;
    emit request(command);
}
void Window::translateLibraryLabels() {
    const auto count = files_->selectedItems().size();
    selection_->setText(tr("%1 photos selected").arg(count));
    int visible = 0;
    for (int i = 0; i < files_->count(); ++i) visible += !files_->item(i)->isHidden();
    gridCount_->setText(tr("%1 of %2 photos · %3 selected").arg(visible).arg(files_->count()).arg(count));
    filmCount_->setText(
        tr("%1    %2 photos").arg(activeAlbumId_ ? activeAlbumName_ : QFileInfo(folder_).fileName()).arg(files_->count()));
    librarySelection_->setText(count > 1 ? tr("%1 photos selected").arg(count)
                                         : QFileInfo(path_).fileName());
    folderTitle_->setText(activeAlbumId_ ? tr("Album: %1").arg(activeAlbumName_)
        : folder_.isEmpty() ? tr("No folder open") : folder_);
    if (open_)
        breadcrumb_->setText(tr("Pictures    /    %1    /    %2")
            .arg(QFileInfo(folder_).fileName(), QFileInfo(path_).fileName()));
}
void Window::translateDocumentStatus() {
    // Never re-enter updateState: it also synchronizes controls and autosaves.
    if (history_->count()) history_->item(0)->setText(tr("Original"));
    if (!statusBar()->currentMessage().isEmpty()) return;
    if (!open_) {
        if (files_->count()) status_->setText(tr("%1 photos").arg(files_->count()));
        else status_->setText(tr("Open a photo or folder to begin"));
        return;
    }
    const auto load = state_.load;
    if (load.status == ImageLoad::Status::Loading) status_->setText(tr("Loading…"));
    else if (load.status == ImageLoad::Status::Failed) status_->setText(load.error);
    else status_->setText(state_.retouchPreview
        ? tr("Retouch preview · release to apply, Escape to cancel")
        : state_.dirty ? tr("Saving…") : tr("All changes saved"));
    const auto exported = state_.exporting;
    if (exported.running) status_->setText(tr("Exporting…"));
    if (exported.error.has_value()) status_->setText(tr("Export failed: %1").arg(*exported.error));
    if (exported.hasResult) status_->setText(tr("Export complete"));
}
void Window::updateSelection() {
    const auto count = files_->selectedItems().size();
    if (export_) export_->setEnabled(open_ || count > 0);
    if (exportButton_) exportButton_->setEnabled(open_ || count > 0);
    translateLibraryLabels();
    if (count > 1) {
        const auto selected = files_->selectedItems();
        const int first = selected.first()->data(PhotoDelegate::RatingRole).toInt();
        const bool same = std::all_of(selected.begin(), selected.end(), [first](auto* item) {
            return item->data(PhotoDelegate::RatingRole).isValid() &&
                   item->data(PhotoDelegate::RatingRole).toInt() == first;
        });
        const auto stars =
            libraryMetadata_->findChild<QWidget*>("ratingStars")->findChildren<QToolButton*>();
        for (int i = 0; i < stars.size(); ++i) {
            stars[i]->setText(same && i < first ? QString::fromUtf8("★") : QString::fromUtf8("☆"));
            stars[i]->setToolTip(tr("Set %1 stars on all %2 selected photos%3")
                                     .arg(i + 1)
                                     .arg(count)
                                     .arg(same ? QString{} : tr(" (mixed or unknown ratings)")));
        }
    }
    else {
        const auto stars =
            libraryMetadata_->findChild<QWidget*>("ratingStars")->findChildren<QToolButton*>();
        for (int i = 0; i < stars.size(); ++i) {
            stars[i]->setText(i < libraryRating_->value() ? QString::fromUtf8("★")
                                                        : QString::fromUtf8("☆"));
            stars[i]->setToolTip(tr("Rate %1 stars; click again to clear").arg(i + 1));
        }
    }
    libraryMetadata_->findChild<QPushButton*>("batchMetadataActions")->setEnabled(count > 0);
    // Rating stars and metadata fields follow the selected photo identities.
    libraryRating_->setEnabled(count <= 1 && open_);
    libraryMetadata_->findChild<QWidget*>("ratingStars")->setEnabled(count > 0);
    QList<PhotoIdentity> targets;
    QList<SelectionPhotoMetadata> metadata;
    for (auto* item : files_->selectedItems()) {
        targets.append(photoIdentity(item));
        metadata.append(selectionMetadataFromJson(item->data(PhotoDelegate::MetadataRole).toJsonObject()));
    }
    selectionMetadata_->setSelection(targets, metadata);
    libraryRating_->setToolTip(count > 1 ? tr("Click a star to rate all selected photos")
                                         : QString{});
    const auto* current = files_->currentItem();
    if (current) {
        const auto image = qvariant_cast<QImage>(current->data(PhotoDelegate::ImageRole));
        if (!image.isNull())
            libraryPreview_->setPixmap(QPixmap::fromImage(image).scaled(
                libraryPreview_->size(), Qt::KeepAspectRatio, Qt::SmoothTransformation));
    }
}
void Window::manageSavedSearches() {
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Saved searches"));
    dialog.resize(440, 340);
    auto* layout = new QVBoxLayout(&dialog);
    auto* hint = new QLabel(tr("Searches apply to the current folder or album. All metadata "
                               "filters, search terms, and sort order are saved."));
    hint->setWordWrap(true);
    layout->addWidget(hint);
    auto* list = new QListWidget;
    list->setObjectName("savedSearchList");
    list->setAccessibleName(tr("Saved searches"));
    layout->addWidget(list);
    auto entries = settings_.value("library/savedSearches").toList();
    const auto refresh = [&] {
        list->clear();
        for (const auto& entry : entries)
            list->addItem(entry.toMap().value("name").toString());
    };
    refresh();
    const auto persist = [&] {
        settings_.setValue("library/savedSearches", entries);
        settings_.sync();
        if (settings_.status() != QSettings::NoError)
            QMessageBox::warning(&dialog, tr("Saved searches"),
                                 tr("Could not save searches to settings."));
        refresh();
    };
    auto* actions = new QHBoxLayout;
    layout->addLayout(actions);
    auto* save = new QPushButton(tr("Save current…"));
    auto* rename = new QPushButton(tr("Rename…"));
    auto* remove = new QPushButton(tr("Delete"));
    auto* open = new QPushButton(tr("Open"));
    for (auto* button : {save, rename, remove, open})
        actions->addWidget(button);
    open->setObjectName("openSavedSearch");
    bindTranslation(&dialog, [&dialog, hint, list, save, rename, remove, open] {
        dialog.setWindowTitle(tr("Saved searches"));
        hint->setText(tr("Searches apply to the current folder or album. All metadata "
                         "filters, search terms, and sort order are saved."));
        save->setText(tr("Save current…"));
        rename->setText(tr("Rename…"));
        remove->setText(tr("Delete"));
        open->setText(tr("Open"));
        list->setAccessibleName(tr("Saved searches"));
    });
    const auto update = [=] {
        for (auto* button : {rename, remove, open})
            button->setEnabled(list->currentRow() >= 0);
    };
    connect(list, &QListWidget::currentRowChanged, &dialog, update);
    update();
    const auto nameFor = [&](const QString& initial, int ignored) {
        bool ok = false;
        QInputDialog editor(&dialog);
        editor.setObjectName("savedSearchNameDialog");
        editor.setWindowTitle(tr("Saved search"));
        editor.setLabelText(tr("Name"));
        editor.setTextValue(initial);
        bindTranslation(&editor, [&editor] {
            editor.setWindowTitle(tr("Saved search"));
            editor.setLabelText(tr("Name"));
        });
        ok = editor.exec() == QDialog::Accepted;
        auto name = ok ? editor.textValue().trimmed() : QString{};
        if (!ok || name.isEmpty())
            return QString{};
        for (int i = 0; i < entries.size(); ++i)
            if (i != ignored && entries[i].toMap().value("name").toString().compare(
                                    name, Qt::CaseInsensitive) == 0) {
                QMessageBox::information(&dialog, tr("Saved search"),
                                         tr("A search with this name already exists."));
                return QString{};
            }
        return name;
    };
    connect(save, &QPushButton::clicked, &dialog, [&] {
        const auto name = nameFor(filter_->text(), -1);
        if (name.isEmpty())
            return;
        libraryFilter_.query = filter_->text();
        entries.append(QVariantMap{
            {"name", name},
            {"filter", toJson(libraryFilter_).toVariantMap()},
            {"sort",
             QVariantMap{
                 {"key",
                  QStringList{"Name", "Rating", "Captured",
                              "Modified"}[findChild<QComboBox*>("librarySort")->currentIndex()]},
                 {"descending", findChild<QToolButton*>("libraryDescending")->isChecked()}}}});
        persist();
        list->setCurrentRow(int(entries.size()) - 1);
    });
    connect(rename, &QPushButton::clicked, &dialog, [&] {
        const int row = list->currentRow();
        if (row < 0)
            return;
        auto entry = entries[row].toMap();
        const auto name = nameFor(entry.value("name").toString(), row);
        if (name.isEmpty())
            return;
        entry["name"] = name;
        entries[row] = entry;
        persist();
        list->setCurrentRow(row);
    });
    connect(remove, &QPushButton::clicked, &dialog, [&] {
        const int row = list->currentRow();
        if (row < 0)
            return;
        entries.removeAt(row);
        persist();
        if (!entries.isEmpty())
            list->setCurrentRow(std::min(row, int(entries.size()) - 1));
    });
    const auto apply = [&] {
        const int row = list->currentRow();
        if (row < 0)
            return;
        const auto entry = entries[row].toMap();
        const auto restoredFilter = QJsonObject::fromVariantMap(entry.value("filter").toMap());
        const auto order = entry.value("sort").toMap();
        const int index = int(QStringList{"Name", "Rating", "Captured", "Modified"}.indexOf(
            order.value("key").toString()));
        if (!validLibrarySearch(QJsonObject::fromVariantMap(entry))) {
            QMessageBox::warning(&dialog, tr("Saved search"),
                                 tr("This search contains unsupported or invalid criteria."));
            return;
        }
        libraryFilter_ = libraryFilterFromJson(restoredFilter);
        const QSignalBlocker searchBlock(filter_);
        auto* sort = findChild<QComboBox*>("librarySort");
        auto* descending = findChild<QToolButton*>("libraryDescending");
        const QSignalBlocker sortBlock(sort), directionBlock(descending);
        filter_->setText(libraryFilter_.query);
        sort->setCurrentIndex(index);
        descending->setChecked(order.value("descending").toBool());
        sortPhotos();
        setMode(false);
        dialog.accept();
    };
    connect(open, &QPushButton::clicked, &dialog, apply);
    connect(list, &QListWidget::itemDoubleClicked, &dialog, apply);
    auto* close = new QDialogButtonBox(QDialogButtonBox::Close);
    layout->addWidget(close);
    connect(close, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    dialog.adjustSize();
    dialog.move(mapToGlobal(rect().center()) - dialog.rect().center());
    dialog.exec();
}
void Window::editLibraryFilters() {
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Library filters"));
    dialog.setMinimumWidth(440);
    auto* form = new QFormLayout(&dialog);
    auto* heading = new QLabel(tr("Library filters"));
    heading->setObjectName("sectionTitle");
    form->addRow(heading);
    auto* rating = new QComboBox;
    rating->setObjectName("filterRating");
    rating->addItems({tr("Any rating / flag"), tr("Unrated"), tr("Picked"), tr("Rejected"),
                      tr("At least 1 star"), tr("At least 2 stars"), tr("At least 3 stars"),
                      tr("At least 4 stars"), tr("5 stars")});
    const auto rv = libraryFilter_.rating;
    rating->setCurrentIndex(rv == LibraryFilter::Rating::AtLeast ? libraryFilter_.minimumRating + 3
        : rv == LibraryFilter::Rating::Unrated ? 1 : rv == LibraryFilter::Rating::Picked ? 2
        : rv == LibraryFilter::Rating::Rejected ? 3 : 0);
    form->addRow(tr("Rating / flag"), rating);
    auto* label = new QComboBox;
    label->setObjectName("filterColorLabel");
    label->setEditable(true);
    label->addItem(tr("Any label"), "Any");
    label->addItem(tr("No label"), "None");
    const QStringList colorNames{tr("Red label"), tr("Yellow label"), tr("Green label"), tr("Blue label"), tr("Purple label")};
    const QStringList colorValues{"red", "yellow", "green", "blue", "purple"};
    for (int i = 0; i < colorNames.size(); ++i) label->addItem(colorNames[i], colorValues[i]);
    const auto lv = libraryFilter_.label;
    if (lv == LibraryFilter::Label::Is) {
        const auto value = libraryFilter_.color;
        const auto index = label->findData(value);
        if (index >= 0) label->setCurrentIndex(index);
        else label->setCurrentText(value);
    }
    else
        label->setCurrentIndex(lv == LibraryFilter::Label::None ? 1 : 0);
    form->addRow(tr("Color label"), label);
    QMap<QString, QComboBox*> fields;
    const QList<QPair<QString, QString>> definitions{{"camera", tr("Camera")},
                                                     {"lens", tr("Lens")},
                                                     {"date", tr("Capture date prefix")},
                                                     {"keyword", tr("Keyword")}};
    for (const auto& [key, title] : definitions) {
        auto* input = new QComboBox;
        input->setEditable(true);
        input->addItem(QString{});
        QStringList values;
        for (int i = 0; i < files_->count(); ++i) {
            const auto meta = files_->item(i)->data(PhotoDelegate::MetadataRole).toJsonObject();
            if (key == "keyword") {
                for (const auto& tag : meta["keywords"].toArray())
                    values.append(tag.toString());
            } else
                values.append(meta[key == "date" ? "captured" : key].toString().left(
                    key == "date" ? 10 : -1));
        }
        values.removeAll(QString{});
        values.removeDuplicates();
        values.sort(Qt::CaseInsensitive);
        input->addItems(values);
        input->setCurrentText(libraryFilter_.facet(key));
        input->setToolTip(key == "date" ? tr("YYYY, YYYY-MM, or YYYY-MM-DD; blank means any")
                                        : tr("Exact value; blank means any"));
        input->setObjectName("filter_" + key);
        fields[key] = input;
        form->addRow(title, input);
    }
    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel |
                                         QDialogButtonBox::Reset);
    form->addRow(buttons);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    connect(buttons->button(QDialogButtonBox::Reset), &QPushButton::clicked, &dialog, [&] {
        rating->setCurrentIndex(0);
        label->setCurrentIndex(0);
        label->setEditText(label->itemText(0));
        for (auto* input : fields)
            input->setCurrentText({});
    });
    bindTranslation(&dialog, [&] {
        dialog.setWindowTitle(tr("Library filters"));
        heading->setText(dialog.windowTitle());
        const QSignalBlocker ratingBlock(rating), labelBlock(label), textBlock(label->lineEdit());
        const auto draft = label->currentText();
        const bool custom = label->currentIndex() < 0 || draft != label->itemText(label->currentIndex());
        const auto cursor = label->lineEdit()->cursorPosition();
        const auto selection = label->lineEdit()->selectionStart();
        const auto selectionLength = label->lineEdit()->selectedText().size();
        const QStringList ratings{tr("Any rating / flag"), tr("Unrated"), tr("Picked"), tr("Rejected"),
            tr("At least 1 star"), tr("At least 2 stars"), tr("At least 3 stars"), tr("At least 4 stars"), tr("5 stars")};
        for (int i = 0; i < ratings.size(); ++i) rating->setItemText(i, ratings[i]);
        const QStringList labels{tr("Any label"), tr("No label"), tr("Red label"), tr("Yellow label"),
            tr("Green label"), tr("Blue label"), tr("Purple label")};
        for (int i = 0; i < labels.size(); ++i) label->setItemText(i, labels[i]);
        if (custom) {
            label->setCurrentIndex(-1);
            label->setEditText(draft);
            label->lineEdit()->setCursorPosition(cursor);
            if (selection >= 0) label->lineEdit()->setSelection(selection, int(selectionLength));
        }
        const auto caption = [form](QWidget* field, const QString& text) {
            qobject_cast<QLabel*>(form->labelForField(field))->setText(text);
            field->setAccessibleName(text);
        };
        caption(rating, tr("Rating / flag"));
        caption(label, tr("Color label"));
        caption(fields["camera"], tr("Camera"));
        caption(fields["lens"], tr("Lens"));
        caption(fields["date"], tr("Capture date prefix"));
        caption(fields["keyword"], tr("Keyword"));
        for (auto it = fields.begin(); it != fields.end(); ++it)
            it.value()->setToolTip(it.key() == "date" ? tr("YYYY, YYYY-MM, or YYYY-MM-DD; blank means any")
                                                     : tr("Exact value; blank means any"));
    });
    dialog.adjustSize();
    dialog.move(mapToGlobal(rect().center()) - dialog.rect().center());
    if (dialog.exec() != QDialog::Accepted)
        return;
    const int r = rating->currentIndex();
    libraryFilter_.rating = r >= 4 ? LibraryFilter::Rating::AtLeast : static_cast<LibraryFilter::Rating>(r);
    libraryFilter_.minimumRating = r >= 4 ? r - 3 : 0;
    const int labelIndex = label->currentIndex() >= 0 &&
        label->currentText() == label->itemText(label->currentIndex())
        ? label->currentIndex() : -1;
    libraryFilter_.label = labelIndex == 0 ? LibraryFilter::Label::Any : labelIndex == 1 ? LibraryFilter::Label::None : LibraryFilter::Label::Is;
    libraryFilter_.color = labelIndex >= 2 ? label->currentData().toString() : label->currentText();
    for (auto it = fields.begin(); it != fields.end(); ++it)
        libraryFilter_.setFacet(it.key(), it.value()->currentText().trimmed());
    filterPhotos();
}
void Window::sortPhotos() {
    const QSignalBlocker blocker(files_);
    const auto selected = files_->selectedItems();
    auto* current = files_->currentItem();
    QList<QListWidgetItem*> items;
    while (files_->count())
        items.append(files_->takeItem(0));
    const auto key = QStringList{"Name", "Rating", "Captured",
                                 "Modified"}[findChild<QComboBox*>("librarySort")->currentIndex()];
    const bool descending = findChild<QToolButton*>("libraryDescending")->isChecked();
    std::stable_sort(items.begin(), items.end(), [&](auto* a, auto* b) {
        // The displayed filename is the primary tie-breaker, with full path for duplicates.
        return libraryLess(a->text() + '\n' + a->data(Qt::UserRole).toString(),
                           libraryPhotoFromJson(a->data(PhotoDelegate::MetadataRole).toJsonObject()),
                           b->text() + '\n' + b->data(Qt::UserRole).toString(),
                           libraryPhotoFromJson(b->data(PhotoDelegate::MetadataRole).toJsonObject()), key, descending);
    });
    for (auto* item : items)
        files_->addItem(item);
    files_->setCurrentItem(current, QItemSelectionModel::NoUpdate);
    for (auto* item : selected)
        item->setSelected(true);
    filterPhotos();
}
void Window::filterPhotos() {
    libraryFilter_.query = filter_->text();
    const QSignalBlocker blocker(files_);
    QSet<QString> seenStacks;
    QHash<QString, int> stackCounts;
    for (int i = 0; i < files_->count(); ++i) {
        const auto stack = files_->item(i)->data(PhotoDelegate::MetadataRole).toJsonObject()["stack"].toString();
        if (!stack.isEmpty()) ++stackCounts[stack];
    }
    for (int i = 0; i < files_->count(); ++i) {
        auto* item = files_->item(i);
        bool matches =
            libraryMatches(item->data(Qt::UserRole).toString(),
                           libraryPhotoFromJson(item->data(PhotoDelegate::MetadataRole).toJsonObject()), libraryFilter_);
        const auto stack = item->data(PhotoDelegate::MetadataRole).toJsonObject()["stack"].toString();
        matches = libraryStackVisible(matches, stack, collapseStacks_, seenStacks);
        item->setData(PhotoDelegate::StackCountRole, stackCounts.value(stack));
        item->setHidden(!matches);
        if (!matches)
            item->setSelected(false);
    }
    updateSelection();
}
void Window::showInspector(const QString& page) {
    inspectorPage_ = page;
    for (auto* b : canvasTools_)
        b->setChecked(b->property("tool").toString() == page);
    for (auto* b : rightRail_->findChildren<QToolButton*>("workspaceTab"))
        b->setChecked(b->property("inspectorMode").toInt() == (page == "agent" ? 1 : 0));
    rightRail_->findChild<QWidget*>("inspectorFooter")->setVisible(page != "agent");
    if (page == "agent") {
        rightPages_->setCurrentIndex(1);
        selectTool("pan");
    } else if(page=="heal") {
        rightPages_->setCurrentIndex(3);
        selectTool("retouch");
    } else if (page == "crop" || page == "masks") {
        rightPages_->setCurrentIndex(2);
        toolsPanel_->setPage(page);
        selectTool(page == "crop"                               ? "crop"
                   : params_.value("masks").toArray().isEmpty() ? "pan"
                                                                : "mask_edit");
    } else {
        rightPages_->setCurrentIndex(0);
        adjustments_->showPage(page == "heal" ? 1 : 0);
        selectTool(page == "heal" ? "heal" : "pan");
    }
}
void Window::setMode(bool develop) {
    developMode_ = develop;
    workspace_->setCurrentIndex(develop ? 1 : 0);
    if (gridAction_)
        gridAction_->setChecked(!develop);
    if (developAction_)
        developAction_->setChecked(develop);
    auto* target = develop ? filmstrip_ : gridHolder_;
    target->layout()->addWidget(files_);
    files_->setFlow(QListView::LeftToRight);
    files_->setWrapping(!develop);
    updatePhotoLayout();
    files_->setIconSize(develop ? QSize(112, 68) : QSize(184, 144));
    files_->setVerticalScrollBarPolicy(develop ? Qt::ScrollBarAlwaysOff : Qt::ScrollBarAsNeeded);
    files_->setHorizontalScrollBarPolicy(develop ? Qt::ScrollBarAsNeeded : Qt::ScrollBarAlwaysOff);
    files_->setVisible(files_->count() > 0);
    updateResponsiveLayout();
    emptyLibrary_->setVisible(files_->count() == 0);
    developContent_->setCurrentIndex(open_ ? 1 : 0);
}
void Window::selectTool(const QString& id) {
    tool_->setCurrentIndex(tool_->findData(id));
    canvas_->setTool(id);
    if (open_)
        emit request(commands::ToolMode{.tool = id});
}
void Window::refreshPresets() {
    presets_->clear();
    const QDir directory(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation) +
                         "/aigle/presets");
    for (const auto& file : directory.entryInfoList({"*.json"}, QDir::Files, QDir::Name)) {
        QFile content(file.absoluteFilePath());
        QString name = file.completeBaseName();
        if (content.open(QIODevice::ReadOnly))
            name = QJsonDocument::fromJson(content.readAll()).object().value("name").toString(name);
        auto* item = new QListWidgetItem(name, presets_);
        item->setData(Qt::UserRole, file.absoluteFilePath());
    }
    if (presets_->count() == 0) {
        auto* empty = new QListWidgetItem(tr("No saved presets"), presets_);
        empty->setFlags(Qt::NoItemFlags);
        empty->setForeground(palette().placeholderText());
    }
    presets_->setMinimumHeight(24);
}
void Window::applyPreset(const QString& path) {
    if (!open_)
        return;
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) {
        showError(file.errorString());
        return;
    }
    const auto document = QJsonDocument::fromJson(file.readAll());
    if (!document.isObject()) {
        showError(tr("Invalid preset JSON"));
        return;
    }
    auto params = document.object();
    if (params.contains("params"))
        params = params.value("params").toObject();
    emit request(commands::Edit{.label = tr("Preset: %1").arg(QFileInfo(path).completeBaseName()), .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::ReplaceAll, {}, params}}});
}

void Window::buildMenus() {
    auto* file = appMenus_->addMenu(tr("&File"));
    bindTranslation(file, [this, menu = file] { menu->setTitle(tr("&File")); });
    file->addAction(tr("Open &Photo…"), QKeySequence::Open, this, [this] {
        const auto path = QFileDialog::getOpenFileName(this, tr("Open photo"), folder_);
        if (!path.isEmpty())
            open(path);
    });
    file->addAction(tr("Open &Folder…"), QKeySequence("Ctrl+Shift+O"), this, [this] {
        const auto path = QFileDialog::getExistingDirectory(this, tr("Open folder"), folder_);
        if (!path.isEmpty())
            openFolder(path);
    });
    file->addAction(tr("&Save"), QKeySequence::Save, this,
                    [this] { emit request(commands::Save{}); });
    export_ =
        file->addAction(tr("&Export…"), QKeySequence("Ctrl+Shift+E"), this, &Window::exportPhoto);
    export_->setEnabled(false);
    file->addAction(tr("Export jobs…"), this, &Window::showExportQueue);
    file->addAction(tr("Cancel exports"), this, &Window::cancelExports);
    file->addSeparator();
    file->addAction(tr("&Quit"), QKeySequence::Quit, this, &QWidget::close);
    auto* edit = appMenus_->addMenu(tr("&Edit"));
    bindTranslation(edit, [this, menu = edit] { menu->setTitle(tr("&Edit")); });
    undo_ = edit->addAction(tr("&Undo"), QKeySequence::Undo, this,
                            [this] { emit request(commands::Undo{}); });
    redo_ = edit->addAction(tr("&Redo"), QKeySequence::Redo, this,
                            [this] { emit request(commands::Redo{}); });
    undo_->setEnabled(false);
    redo_->setEnabled(false);
    edit->addSeparator();
    edit->addAction(tr("Reset all adjustments"), this, [this] {
        if (open_)
            emit request(commands::Edit{.label = "Reset all", .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::Reset, "", {}}}});
    });
    edit->addAction(tr("Auto white balance"), this, [this] {
        if (open_)
            emit request(commands::AutoWb{});
    });
    edit->addAction(tr("Auto tone"), this, [this] {
        if (open_)
            emit request(commands::AutoTone{});
    });
    edit->addSeparator();
    edit->addAction(tr("Load preset…"), this, &Window::loadPreset);
    edit->addAction(tr("Save preset…"), this, &Window::savePreset);
    edit->addAction(tr("Copy selected adjustments…"), QKeySequence("Ctrl+Shift+C"), this,
                    [this] { transferAdjustments(false); });
    edit->addAction(tr("Paste selected adjustments"), QKeySequence("Ctrl+Shift+V"), this, [this] {
        if (!open_) return;
        const auto bytes = QApplication::clipboard()->mimeData()->data("application/x-aigle-selected-edits");
        auto payload = QJsonDocument::fromJson(bytes).object();
        if (payload.isEmpty()) {
            statusBar()->showMessage(tr("Copy adjustment groups first"), 5000);
            return;
        }
        emit request(commands::PasteSelected{.params = payload["params"].toObject(), .groups = commandStringsFromJson(payload["groups"].toArray())});
    });
    edit->addAction(tr("Sync selected adjustments to selection…"), this,
                    [this] { transferAdjustments(true); });
    edit->addAction(tr("Preferences…"), this, &Window::editPreferences);
    edit->addAction(tr("Keyboard shortcuts…"), this, &Window::editShortcuts);
    auto* photo = appMenus_->addMenu(tr("Photo"));
    bindTranslation(photo, [this, menu = photo] { menu->setTitle(tr("Photo")); });
    photo->addAction(tr("Create snapshot…"), this, [this] {
        if (!open_)
            return;
        const auto name = QInputDialog::getText(this, tr("Create snapshot"), tr("Name"));
        if (!name.trimmed().isEmpty())
            emit request(commands::Snapshot{.name = name});
    });
    photo->addAction(tr("Restore snapshot…"), this, [this] {
        QStringList names;
        for (const auto& name : state_.snapshots)
            names.append(name);
        if (names.isEmpty())
            return;
        bool accepted = false;
        const auto selected = QInputDialog::getItem(this, tr("Restore snapshot"), tr("Snapshot"),
                                                    names, 0, false, &accepted);
        if (accepted)
            emit request(commands::RestoreSnapshot{.index = names.indexOf(selected)});
    });
    photo->addAction(tr("Create virtual copy…"), this, [this] {
        QList<PhotoIdentity> targets;
        if (!developMode_)
            for (auto* item : files_->selectedItems()) targets.append(photoIdentity(item));
        if (developMode_ ? !open_ : targets.isEmpty()) return;
        const auto name = QInputDialog::getText(this, tr("Create virtual copy"), tr("Name"));
        if (!name.trimmed().isEmpty())
            emit request(developMode_ ? EngineCommand{commands::CreateCopy{.name = name}}
                                     : EngineCommand{commands::CreateCopies{.name = name, .targets = targets}});
    });
    photo->addAction(tr("Open virtual copy…"), this, [this] {
        if (open_)
            emit request(commands::Copies{});
    });
    photo->addAction(tr("Rename current virtual copy…"), this, [this] {
        if ((state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null)).isNull() || (state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null)).isUndefined()) {
            statusBar()->showMessage(tr("Open a virtual copy first"), 5000);
            return;
        }
        const QJsonValue id = (state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null));
        const auto name = QInputDialog::getText(this, tr("Rename virtual copy"), tr("Name"));
        if (!name.trimmed().isEmpty())
            emit request(commands::RenameCopy{.id = id, .name = name});
    });
    photo->addAction(tr("Delete current virtual copy…"), this, [this] {
        if ((state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null)).isNull() || (state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null)).isUndefined()) {
            statusBar()->showMessage(tr("Open a virtual copy first"), 5000);
            return;
        }
        const QJsonValue id = (state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null));
        if (QMessageBox::question(this, tr("Delete virtual copy"),
                tr("Delete this virtual copy and its edit history? The master photo and its edits will be preserved."),
                QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel) == QMessageBox::Yes)
            emit request(commands::RemoveCopy{.id = id});
    });
    photo->addAction(tr("Open master photo"), this, [this] {
        if (open_)
            open(path_);
    });
    auto* library = new QMenu(tr("Library"), this);
    bindTranslation(library, [this, library] { library->setTitle(tr("Library")); });
    file->addMenu(library);
    library->addAction(tr("Import photos…"), this, [this] {
        const auto source = QFileDialog::getExistingDirectory(this, tr("Import from folder"), settings_.value("import/source").toString());
        if (source.isEmpty())
            return;
        const auto destination = QFileDialog::getExistingDirectory(this, tr("Destination folder"), settings_.value("import/destination").toString());
        if (destination.isEmpty())
            return;
        QDialog dialog(this);
        dialog.setWindowTitle(tr("Import photos"));
        auto* form = new QFormLayout(&dialog);
        auto *folders = new QLineEdit(settings_.value("import/folders", "{year}/{year}-{month}-{day}").toString(), &dialog),
             *names = new QLineEdit(settings_.value("import/names", "{name}").toString(), &dialog),
             *backup = new QLineEdit(settings_.value("import/backup").toString(), &dialog);
        folders->setToolTip(tr("Date folders; leave empty to copy directly into the destination"));
        backup->setPlaceholderText(tr("Optional second destination"));
        form->addRow(tr("Subfolders"), folders);
        form->addRow(tr("File names"), names);
        form->addRow(tr("Backup folder"), backup);
        folders->setObjectName("importFolders");
        names->setObjectName("importNames");
        backup->setObjectName("importBackup");
        bindTranslation(&dialog, [this, &dialog, form, folders, names, backup] {
            dialog.setWindowTitle(tr("Import photos"));
            folders->setToolTip(tr("Date folders; leave empty to copy directly into the destination"));
            backup->setPlaceholderText(tr("Optional second destination"));
            const auto label = [form](QLineEdit* field, const QString& text) {
                qobject_cast<QLabel*>(form->labelForField(field))->setText(text);
                field->setAccessibleName(text);
            };
            label(folders, tr("Subfolders"));
            label(names, tr("File names"));
            label(backup, tr("Backup folder"));
        });
        auto* buttons =
            new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
        form->addRow(buttons);
        connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
        connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
        if (dialog.exec() == QDialog::Accepted) {
            settings_.setValue("import/source", source);
            settings_.setValue("import/destination", destination);
            settings_.setValue("import/folders", folders->text());
            settings_.setValue("import/names", names->text());
            settings_.setValue("import/backup", backup->text());
            settings_.sync();
            emit request(commands::Import{.source = source, .destination = destination, .folders = folders->text(), .names = names->text(), .backup = backup->text()});
        }
    });
    library->addAction(tr("Cancel import"), this,
                       [this] { emit request(commands::CancelImport{}); });
    library->addAction(tr("Create album from selection…"), this, [this] {
        QStringList paths;
        for (auto* item : files_->selectedItems())
            paths.append(item->data(Qt::UserRole).toString());
        if (paths.isEmpty())
            return;
        const auto name = QInputDialog::getText(this, tr("Create album"), tr("Name"));
        if (!name.trimmed().isEmpty())
            emit request(commands::CreateAlbum{.name = name, .paths = paths});
    });
    library->addAction(tr("Open album…"), this, [this] { emit request(commands::Albums{}); });
    library->addAction(tr("Add selection to album…"), this, [this] {
        QStringList paths;
        for (auto* item : files_->selectedItems()) paths.append(item->data(Qt::UserRole).toString());
        if (!paths.isEmpty()) emit request(commands::Albums{.action = "add", .paths = paths});
        else statusBar()->showMessage(tr("Select photos to add to an album"), 5000);
    });
    library->addAction(tr("Remove selection from current album"), this, [this] {
        if (!activeAlbumId_) {
            statusBar()->showMessage(tr("Open an album first"), 5000);
            return;
        }
        QStringList paths;
        for (auto* item : files_->selectedItems()) paths.append(item->data(Qt::UserRole).toString());
        if (!paths.isEmpty()) emit request(commands::UpdateAlbum{.id = activeAlbumId_, .paths = paths, .add = false});
    });
    library->addAction(tr("Rename album…"), this, [this] {
        emit request(commands::Albums{.action = "rename"});
    });
    library->addAction(tr("Delete album…"), this, [this] {
        emit request(commands::Albums{.action = "delete"});
    });
    library->addSeparator();
    library->addAction(tr("Stack selected photos"), this, [this] { stackSelection(false); });
    library->addAction(tr("Unstack selected stacks"), this, [this] { stackSelection(true); });
    auto* collapseStacks = library->addAction(tr("Collapse stacks"));
    collapseStacks->setCheckable(true);
    connect(collapseStacks, &QAction::toggled, this, [this](bool enabled) {
        collapseStacks_ = enabled;
        filterPhotos();
    });
    library->addAction(tr("Find exact duplicates…"), this, &Window::findDuplicates);
    library->addAction(tr("Compare selected photos…"), this, [this] {
        QList<PhotoIdentity> paths;
        for (auto* item : files_->selectedItems()) {
            auto identity = photoIdentity(item);
            identity.name = item->text();
            paths.append(identity);
        }
        if (paths.size() < 2 || paths.size() > 4) {
            statusBar()->showMessage(tr("Select two to four photos to compare"), 5000);
            return;
        }
        showComparison(this, engine_, paths, [this](const PhotoIdentity& target, const QString& flag) {
            emit request(commands::BatchMetadata{.targets = QList<PhotoIdentity>{target}, .flag = flag});
        }, canvas_);
    });
    library->addSeparator();
    library->addAction(tr("Rename selected photo…"), this, [this] { startFileActions("rename"); });
    library->addAction(tr("Move selected photos…"), this, [this] { startFileActions("move"); });
    library->addAction(tr("Move selected photos to Trash…"), this, [this] { startFileActions("trash"); });
    library->addAction(tr("Locate missing original…"), this, [this] { startFileActions("relink"); });
    library->addAction(tr("Manage saved searches…"), this, &Window::manageSavedSearches);
    library->addAction(tr("Back up catalog…"), this, [this] {
        const auto path =
            QFileDialog::getSaveFileName(this, tr("Back up catalog"), "aigle-catalog.sqlite3");
        if (!path.isEmpty())
            emit request(commands::CatalogBackup{.path = path});
    });
    library->addAction(tr("Restore catalog…"), this, [this] {
        if (exportQueue_.hasPending() || fileProgress_) {
            QMessageBox::information(this, tr("Restore catalog"), tr("Finish or cancel pending export and file jobs first."));
            return;
        }
        const auto path = QFileDialog::getOpenFileName(this, tr("Restore catalog backup"),
            settings_.value("catalog/lastRecoveryBackup").toString(), tr("Catalog backups (*.sqlite3 *.db);;All files (*)"));
        if (path.isEmpty()) return;
        QMessageBox review(QMessageBox::Warning, tr("Restore catalog"),
            tr("Replace catalog history, virtual copies, and albums with this backup? Current edits will be saved and the open photo closed. A recovery backup will be created first. Original photos and XMP sidecars are not restored or replaced."),
            QMessageBox::Ok | QMessageBox::Cancel, this);
        review.setDetailedText(path);
        review.setDefaultButton(QMessageBox::Cancel);
        if (review.exec() == QMessageBox::Ok) emit request(commands::CatalogRestore{.path = path});
    });
    auto* view = appMenus_->addMenu(tr("&View"));
    bindTranslation(view, [this, menu = view] { menu->setTitle(tr("&View")); });
    view->addAction(tr("Fit image"), QKeySequence("F"), canvas_, &Canvas::fit);
    view->addAction(tr("Actual pixels"), QKeySequence("1"), canvas_, &Canvas::oneToOne);
    hdr_ = view->addAction(tr("HDR canvas"));
    hdr_->setCheckable(true);
    hdr_->setShortcut(QKeySequence("Ctrl+Shift+H"));
    connect(hdr_, &QAction::toggled, canvas_, &Canvas::setHdr);
    auto* original = view->addAction(tr("Show original"));
    original->setCheckable(true);
    original->setShortcut(QKeySequence("\\"));
    connect(original, &QAction::toggled, canvas_, &Canvas::setOriginal);
    auto* compare = view->addAction(tr("Before / after"));
    compare->setCheckable(true);
    compare->setShortcut(QKeySequence("Y"));
    connect(compare, &QAction::toggled, canvas_, &Canvas::setCompare);
    connect(original, &QAction::toggled, this, [compare](bool on) {
        if (on) compare->setChecked(false);
    });
    connect(compare, &QAction::toggled, this, [original](bool on) {
        if (on) original->setChecked(false);
    });
    compareButton_->setDefaultAction(compare);
    auto* clipping = view->addAction(tr("Clipping overlay"));
    clipping->setCheckable(true);
    clipping->setShortcut(QKeySequence("J"));
    connect(clipping, &QAction::toggled, this, [this](bool on) {
        updateOverlays({.clipping = on});
    });
    auto* proofMenu = view->addMenu(tr("Soft proof"));
    bindTranslation(proofMenu, [this, menu = proofMenu] { menu->setTitle(tr("Soft proof")); });
    auto* proof = proofMenu->addAction(tr("Enable soft proof"));
    proof->setCheckable(true);
    auto* gamut = proofMenu->addAction(tr("Gamut warning"));
    gamut->setCheckable(true);
    auto* targets = new QActionGroup(this);
    targets->setExclusive(true);
    for (const auto& target : {QString("Srgb"), QString("DisplayP3"), QString("AdobeRgb")}) {
        auto* action = proofMenu->addAction(target == "Srgb" ? "sRGB" : target == "DisplayP3" ? "Display P3" : "Adobe RGB");
        action->setData(target);
        action->setProperty("commandId", "proof_" + target);
        action->setCheckable(true);
        action->setChecked(target == "Srgb");
        targets->addAction(action);
        connect(action, &QAction::triggered, this, [this, proof, target] {
            proof->setChecked(true);
            updateOverlays({.proof = std::optional<QString>{target}});
        });
    }
    connect(proof, &QAction::toggled, this, [this, targets, gamut](bool on) {
        if (!on) gamut->setChecked(false);
        updateOverlays({.proof = on ? std::optional<QString>{targets->checkedAction()->data().toString()} : std::optional<QString>{}});
    });
    connect(gamut, &QAction::toggled, this, [this, proof](bool on) {
        if (on) proof->setChecked(true);
        updateOverlays({.gamutWarning = on});
    });
    view->addSeparator();
    auto* history = view->addAction(tr("History"));
    history->setCheckable(true);
    history->setChecked(true);
    connect(history, &QAction::toggled, historyPane_, &QWidget::setVisible);
    auto* metadata = view->addAction(tr("Metadata sidebar"));
    metadata->setCheckable(true);
    metadata->setChecked(true);
    connect(metadata, &QAction::toggled, libraryMetadata_, &QWidget::setVisible);
    auto* help = appMenus_->addMenu(tr("Help"));
    bindTranslation(help, [this, menu = help] { menu->setTitle(tr("Help")); });
    appMenus_->removeAction(photo->menuAction());
    appMenus_->insertMenu(help->menuAction(), photo);
    help->addAction(tr("About Aigle"), this, [this] {
        QMessageBox::about(
            this, tr("Aigle"),
            tr("Aigle · Qt 6 editor\nC++23 interface with the Rust image engine\nAGPL-3.0-only"));
    });
    tool_ = new QComboBox(this);
    tool_->hide();
    auto* tools = edit->addMenu(tr("Tools"));
    bindTranslation(tools, [this, menu = tools] { menu->setTitle(tr("Tools")); });
    auto* toolActions = new QActionGroup(this);
    const std::array<std::tuple<QString, QString, QString>, 9> definitions{
        {{tr("Pan"), "pan", "view"},
         {tr("Crop"), "crop", "select"},
         {tr("White balance picker"), "wb", "picker"},
         {tr("Heal spot"), "heal", "heal"},
         {tr("Clone spot"), "clone", "clone"},
         {tr("Red eye"), "red_eye", "camera"},
         {tr("Radial mask"), "radial", "view"},
         {tr("Linear mask"), "linear", "gradient"},
         {tr("Brush mask"), "brush", "brush"}}};
    for (const auto& [name, id, icon] : definitions) {
        tool_->addItem(name, id);
        auto* action = tools->addAction(glyph(icon), name, this, [this, id] {
            if(id=="heal"||id=="clone"||id=="red_eye"){showInspector("heal");retouch_->beginNew(id);}else selectTool(id);
        });
        action->setCheckable(true);
        action->setChecked(id == "pan");
        toolActions->addAction(action);
    }
    tool_->addItem(tr("Color range mask"), "color_range");
    tool_->addItem(tr("Sample mask color"), "color_pick");
    tool_->addItem(tr("Edit mask"), "mask_edit");
    tool_->addItem(tr("Retouch corrections"), "retouch");
    tools->addSeparator();
    tools->addAction(tr("Brush radius…"), this, [this] {
        bool accepted = false;
        const auto radius = QInputDialog::getDouble(
            this, tr("Brush radius"), tr("Percent of image long edge"), 2, 0.05, 25, 2, &accepted);
        if (accepted) {
            if (inspectorPage_=="heal") retouch_->setRadius(radius/100);
            else canvas_->setRadius(radius/100);
        }
    });
    gridAction_ = view->addAction(glyph("grid"), tr("Library"), QKeySequence("G"), this,
                                  [this] { setMode(false); });
    developAction_ =
        view->addAction(glyph("develop"), tr("Develop"), QKeySequence("D"), this, [this] {
            if (!open_ && files_->currentItem())
                open(files_->currentItem()->data(Qt::UserRole).toString());
            else
                setMode(true);
        });
    auto* modes = new QActionGroup(this);
    for (auto* action : {gridAction_, developAction_}) {
        action->setCheckable(true);
        modes->addAction(action);
    }
    gridButton_->setDefaultAction(gridAction_);
    developButton_->setDefaultAction(developAction_);
    auto* queue = new QMenu(queueButton_);
    queue->addAction(export_);
    queue->addAction(tr("Export jobs…"), this, &Window::showExportQueue);
    queue->addAction(tr("Pause after current photo"), this, [this] { exportQueue_.paused = true; });
    queue->addAction(tr("Resume exports"), this, [this] {
        exportQueue_.paused = false;
        advanceExportQueue();
    });
    queue->addAction(tr("Cancel exports"), this, &Window::cancelExports);
    queueButton_->setMenu(queue);
}

void Window::open(const QString& path, bool develop, qint64 copy) {
    pendingPath_ = QFileInfo(path).absoluteFilePath();
    bool albumMember = false;
    if (activeAlbumId_)
        for (int i = 0; i < files_->count(); ++i)
            albumMember |= files_->item(i)->data(Qt::UserRole).toString() == pendingPath_;
    if (!albumMember && (activeAlbumId_ || QFileInfo(path).absolutePath() != folder_)) {
        const auto nextFolder = QFileInfo(path).absolutePath();
        if (activeAlbumId_ || nextFolder != folder_) {
            libraryFilter_ = libraryFilterForNewFolder(libraryFilter_);
            filterPhotos();
        }
        folder_ = nextFolder;
        emit request(commands::Scan{.path = folder_, .recursive = preferences_.recursive, .preserve_mode = true});
    }
    status_->setText(tr("Opening %1…").arg(QFileInfo(path).fileName()));
    emit request(commands::Open{.path = pendingPath_, .develop = develop, .copy = copy ? QJsonValue(copy) : QJsonValue(QJsonValue::Null)});
}
void Window::openFolder(const QString& path) {
    if (activeAlbumId_ || path != folder_) {
        libraryFilter_ = libraryFilterForNewFolder(libraryFilter_);
        filterPhotos();
    }
    folder_ = path;
    status_->setText(tr("Reading folder…"));
    emit request(commands::Scan{.path = path, .recursive = preferences_.recursive});
}
void Window::receive(const EngineReply& reply) {
    const auto& cmd = reply.command;
    if (cmd == "tool" && reply.get<ToolResult>().createdMask) {
        toolsPanel_->selectCreatedMask(*reply.get<ToolResult>().createdMask);
        selectTool("mask_edit");
    }
    if (cmd == "import_profile" || cmd == "match_lens") {
        const auto& result = reply.get<ProfileResult>();
        auto message = tr("Selected profile: %1").arg(result.name);
        if (!result.cameraModel.isEmpty())
            message += tr(" · Made for %1").arg(result.cameraModel);
        statusBar()->showMessage(message, 15000);
    }
    if(cmd.startsWith("retouch_")) {
        const auto& result = reply.get<RetouchResult>();
        canvas_->setRetouchResult(result); retouch_->setResult(result);return;
    }
    if (cmd == "scopes") {
        histogram_->setResult(reply.get<ScopeResult>());
        return;
    }
    if (cmd == "agent_models") {
        if (reply.get<ModelDiscovery>().error) emit agent_->modelsFinished(reply.get<ModelDiscovery>());
        return;
    }
    if (cmd == "agent_credential") {
        emit agent_->credentialFinished(reply.get<CredentialResult>());
        return;
    }
    lastError_.clear();
    if (cmd == "catalog_restore") {
        const auto& result = reply.get<CatalogRestoreResult>();
        autosave_.stop();
        path_.clear();
        pendingPath_.clear();
        open_ = false;
        activeAlbumId_ = 0;
        activeAlbumName_.clear();
        canvas_->setFrame({}, false);
        canvas_->setFrame({}, true);
        libraryPreview_->clear();
        libraryInfo_->clear();
        history_->clear();
        snapshots_->clear();
        setMode(false);
        if (!folder_.isEmpty()) openFolder(folder_);
        else receive({"scan", QStringList{}});
        const auto recovery = result.recovery;
        settings_.setValue("catalog/lastRecoveryBackup", recovery);
        if (result.restored)
            QMessageBox::information(this, tr("Catalog restored"), tr("Catalog restored. Originals and sidecars are unchanged.\n\nRecovery backup:\n%1\n\nUse Restore catalog to restore this recovery backup if needed.").arg(recovery));
        else
            QMessageBox::warning(this, tr("Catalog restore failed"), result.error +
                (!result.rollbackError ? tr("\nThe previous catalog was restored.") : tr("\nRecovery also failed: %1").arg(*result.rollbackError)) + tr("\nRecovery backup: %1").arg(recovery));
    } else if (cmd == "catalog_backup") {
        statusBar()->showMessage(tr("Catalog backup saved. Originals and sidecars are separate."), 7000);
    } else if (cmd == "sync_selected") {
        const auto& result = reply.get<BatchResult>();
        for (const auto& photo : result.updated)
            emit request(commands::LibraryMetadata{.path = photo.identity.path, .copy = photo.identity.copy ? QJsonValue(photo.identity.copy) : QJsonValue(QJsonValue::Null)});
        statusBar()->showMessage(tr("Synced adjustments to %1 photos").arg(result.updated.size()), 6000);
        QStringList errors;
        for (const auto& failure : result.failures) errors.append(failure.first + ": " + failure.second);
        if (!errors.isEmpty()) QMessageBox::warning(this, tr("Some photos could not be synced"), errors.join('\n'));
    } else if (cmd == "export_inputs") {
        const auto finish = qScopeGuard([this] { exportPreparing_ = false; updateSelection(); });
        const auto& result = reply.get<ExportInputsResult>();
        if (result.error) showError(*result.error);
        else exportInputs(result.inputs);
    } else if (cmd == "export" && reply.get<ExportProgress>().error) {
        observeExport(reply.get<ExportProgress>());
    } else if (cmd == "file_action") {
        finishFileAction(reply.get<FileActionResult>());
    } else if (cmd == "find_duplicates" && reply.get<DuplicateResult>().error) {
        updateDuplicates(reply.get<DuplicateResult>());
    } else if (cmd == "library_copies") {
        for (const auto& copy : reply.get<QList<CopyRecord>>()) {
            const PhotoIdentity identity{copy.path, copy.id, {}};
            QListWidgetItem* row = nullptr;
            QListWidgetItem* master = nullptr;
            for (int i = 0; i < files_->count(); ++i) {
                auto* item = files_->item(i);
                if (samePhoto(item, identity)) row = item;
                if (item->data(Qt::UserRole).toString() == copy.path &&
                    !item->data(PhotoDelegate::CopyRole).toLongLong()) master = item;
            }
            if (!master) continue; // Ignore replies for a source that has been closed.
            if (!row) {
                row = new QListWidgetItem(files_);
                row->setData(Qt::UserRole, copy.path);
                row->setData(PhotoDelegate::CopyRole, copy.id);
                row->setData(PhotoDelegate::ImageRole, master->data(PhotoDelegate::ImageRole));
            }
            row->setText(tr("%1 (%2)").arg(QFileInfo(copy.path).fileName(), copy.name));
            row->setToolTip(row->text() + "\n" + copy.path);
            emit request(commands::LibraryMetadata{.path = copy.path, .copy = copy.id});
            if (open_ && samePhoto(row, photoIdentityFromJson(QJsonObject{{"path", path_}, {"copy", (state_.copy ? QJsonValue(*state_.copy) : QJsonValue(QJsonValue::Null))}})))
                files_->setCurrentItem(row);
        }
        sortPhotos();
        filterPhotos();
    } else if (cmd == "library_metadata") {
        const auto& photo = reply.get<PhotoMetadata>();
        for (int i = 0; i < files_->count(); ++i) {
            auto* item = files_->item(i);
            if (!samePhoto(item, photo.identity))
                continue;
            item->setData(PhotoDelegate::MetadataRole, toJson(photo));
            if (photo.facts.rating.has_value())
                item->setData(PhotoDelegate::RatingRole, *photo.facts.rating);
            if (photo.error.has_value())
                item->setToolTip(photo.identity.path + "\n" +
                                 *photo.error);
            break;
        }
        if (!libraryViewTimer_.isActive())
            libraryViewTimer_.start();
    } else if (cmd == "batch_metadata") {
        const auto& result = reply.get<BatchResult>();
        if (!result.failures.isEmpty()) selectionMetadata_->failed();
        for (const auto& photo : result.updated) {
            emit request(commands::LibraryMetadata{.path = photo.identity.path, .copy = photo.identity.copy ? QJsonValue(photo.identity.copy) : QJsonValue(QJsonValue::Null)});
            for (int i = 0; i < files_->count(); ++i)
                if (samePhoto(files_->item(i), photo.identity)) files_->item(i)->setData(PhotoDelegate::RatingRole, photo.facts.rating.value_or(0));
        }
        filterPhotos();
        statusBar()->showMessage(tr("Updated %1 photos").arg(result.updated.size()), 5000);
        QStringList errors;
        for (const auto& failure : result.failures) errors.append(failure.first + ": " + failure.second);
        if (!errors.isEmpty()) QMessageBox::warning(this, tr("Some photos could not be updated"), errors.join('\n'));
    } else if (cmd == "schema") {
        adjustments_->setSchema(reply.get<SchemaDocument>().schema);
        exportFormats_ = exportFormatsFromJson(reply.get<SchemaDocument>().schema.value("x-export-formats").toArray());
        QJsonObject fields;
        for (const auto& field : reply.get<SchemaDocument>().schema.value("x-metadata-fields").toArray())
            fields.insert(field.toString(), QJsonObject{{"type", "string"}});
        metadataSchema_ = {{"type", "object"}, {"properties", fields}};
    } else if (cmd == "scan" || cmd == "scan_develop" || cmd == "album_paths") {
        const auto album = cmd == "album_paths" ? reply.get<AlbumRecord>() : AlbumRecord{};
        const bool enteringAlbum = cmd == "album_paths" && activeAlbumId_ == 0;
        const bool leavingAlbum = cmd != "album_paths" && activeAlbumId_ != 0;
        if (enteringAlbum || leavingAlbum)
            libraryFilter_ = libraryFilterForNewFolder(libraryFilter_);
        activeAlbumId_ = cmd == "album_paths" ? album.id : 0;
        activeAlbumName_ = cmd == "album_paths" ? album.name : QString{};
        const auto paths = cmd == "album_paths" ? album.paths : reply.get<QStringList>();
        QStringList thumbnails;
        files_->clear();
        emit request(commands::LibraryCopies{.paths = paths});
        for (const auto& path : paths) {
            auto* item = new QListWidgetItem(QFileInfo(path).fileName(), files_);
            item->setData(Qt::UserRole, path);
            const bool exists = QFileInfo::exists(path);
            item->setToolTip(exists ? path : tr("Missing original: %1").arg(path));
            if (!exists) item->setText(tr("%1 (missing)").arg(item->text()));
            thumbnails.append(path);
        }
        QMetaObject::invokeMethod(
            engine_, [engine = engine_, thumbnails] { engine->thumbnails(thumbnails); },
            Qt::QueuedConnection);
        sortPhotos();
        status_->setText(tr("%1 photos").arg(files_->count()));
        folderTitle_->setText(activeAlbumId_ ? tr("Album: %1").arg(activeAlbumName_) : folder_);
        setMode(cmd == "scan_develop" && developMode_);
    } else if (cmd == "open" || cmd == "synthetic" || cmd == "remote_open") {
        const auto& opened = reply.get<OpenResult>();
        if (cmd == "remote_open")
            pendingPath_ = opened.path;
        path_ = opened.path.isNull() ? pendingPath_ : opened.path;
        open_ = true;
        updateOverlays({.mask = std::optional<qint64>{}});
        params_ = {};
        histogram_->reset();
        adjustments_->setHistogram({});
        canvas_->setRetouchState(0,0,{});retouch_->reset();
        canvas_->fit();
        setMode(opened.develop);
        for (int i = 0; i < files_->count(); ++i)
            if (samePhoto(files_->item(i), PhotoIdentity{path_, opened.copy, {}})) {
                files_->setCurrentRow(i);
                files_->scrollToItem(files_->item(i));
                break;
            }
        emit request(commands::ToolMode{.tool = tool_->currentData().toString()});
        for (const auto& warning : opened.warnings)
            statusBar()->showMessage(warning, 15000);
    } else if (cmd == "save") {
        if (closing_) {
            closeSaved_ = true;
            close();
        }
    } else if (cmd == "tool_mode") {
        canvas_->fit();
    } else if (cmd == "create_copies") {
        const auto& result = reply.get<CreateCopiesResult>();
        receive({"library_copies", result.copies});
        files_->clearSelection();
        for (const auto& copy : result.copies)
            for (int i = 0; i < files_->count(); ++i)
                if (samePhoto(files_->item(i), {copy.path, copy.id, {}})) files_->item(i)->setSelected(true);
        QStringList errors;
        for (const auto& failure : result.errors) errors.append(failure.first + ": " + failure.second);
        if (!errors.isEmpty()) showError(errors.join('\n'));
    } else if (cmd == "create_copy") {
        const auto& copy = reply.get<CopyRecord>();
        pendingPath_ = copy.path;
        emit request(commands::LibraryCopies{.paths = QStringList{pendingPath_}});
        emit request(commands::Open{.path = pendingPath_, .copy = copy.id});
    } else if (cmd == "remove_copy") {
        auto result = reply.get<OpenResult>();
        for (int i = files_->count() - 1; i >= 0; --i)
            if (files_->item(i)->data(PhotoDelegate::CopyRole).toLongLong() == result.id)
                delete files_->takeItem(i);
        result.develop = developMode_;
        receive({"open", result});
        statusBar()->showMessage(tr("Virtual copy deleted; master photo opened"), 5000);
    } else if (cmd == "rename_copy") {
        emit request(commands::LibraryCopies{.paths = QStringList{path_}});
        statusBar()->showMessage(tr("Virtual copy renamed to %1").arg(reply.get<CopyRecord>().name), 5000);
    } else if (cmd == "create_album") {
        emit request(commands::AlbumPaths{.id = reply.get<qint64>()});
    } else if (cmd == "update_album" || cmd == "rename_album") {
        const auto& album = reply.get<AlbumRecord>();
        if (activeAlbumId_ == album.id) receive({"album_paths", album});
        statusBar()->showMessage(tr("Album updated: %1").arg(album.name), 5000);
    } else if (cmd == "delete_album") {
        const auto& album = reply.get<AlbumRecord>();
        if (activeAlbumId_ == album.id) {
            activeAlbumId_ = 0;
            activeAlbumName_.clear();
            if (!folder_.isEmpty()) openFolder(folder_);
            else receive({"scan", QStringList{}});
        }
        statusBar()->showMessage(tr("Album deleted; original photos preserved"), 5000);
    } else if (cmd == "copies" || cmd == "albums") {
        const auto response = cmd == "albums" ? reply.get<AlbumChoices>() : AlbumChoices{};
        QList<CopyRecord> choices;
        if (cmd == "copies") choices = reply.get<QList<CopyRecord>>();
        else for (const auto& album : response.albums) choices.append({album.id, {}, album.name});
        const auto action = response.action;
        QStringList names;
        for (const auto& choice : choices)
            names.append(cmd == "copies" ? tr("%1 (copy %2)").arg(choice.name).arg(choice.id)
                                         : choice.name);
        if (names.isEmpty()) {
            statusBar()->showMessage(cmd == "copies" ? tr("No virtual copies") : tr("No albums"), 5000);
            return;
        }
        bool accepted = false;
        const auto title = cmd == "copies" ? tr("Open virtual copy") : action == "add" ? tr("Add selection to album") :
                           action == "rename" ? tr("Rename album") : action == "delete" ? tr("Delete album") : tr("Open album");
        const auto selected = QInputDialog::getItem(this, title, tr("Name"), names, 0, false, &accepted);
        if (!accepted) return;
        const auto choice = choices[names.indexOf(selected)];
        if (cmd == "copies") {
            pendingPath_ = choice.path;
            emit request(commands::Open{.path = pendingPath_, .copy = choice.id});
        } else {
            const qint64 id = choice.id;
            if (action == "add") emit request(commands::UpdateAlbum{.id = id, .paths = response.paths, .add = true});
            else if (action == "rename") {
                const auto name = QInputDialog::getText(this, title, tr("New name"), QLineEdit::Normal, selected);
                if (!name.trimmed().isEmpty()) emit request(commands::RenameAlbum{.id = id, .name = name});
            } else if (action == "delete") {
                if (QMessageBox::question(this, title,
                        tr("Delete album ‘%1’? Original photos and their edits will be preserved.").arg(selected),
                        QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel) == QMessageBox::Yes)
                    emit request(commands::DeleteAlbum{.id = id});
            } else emit request(commands::AlbumPaths{.id = id});
        }
    }
}
void Window::detectDisplay() {
    detectedDisplay_ = {};
    canvas_->setDisplayProfile({}, tr("sRGB · detecting monitor profile"));
    if (engineReady_) emit request(commands::DetectDisplay{.screen = canvas_->screen() ? canvas_->screen()->name() : QString()});
}
void Window::applyDisplayProfile() {
    const auto screen = canvas_->screen() ? canvas_->screen()->name() : QString();
    const bool detected = preferences_.autoDisplay && !detectedDisplay_.running &&
        detectedDisplay_.screen == screen && detectedDisplay_.profile.has_value();
    canvas_->setDisplayProfile(detected ? *detectedDisplay_.profile : DisplayProfile{},
        detected ? (detectedDisplay_.name.isNull() ? tr("Monitor ICC profile") : detectedDisplay_.name) : tr("sRGB"));
    display_->setToolTip(preferences_.autoDisplay ? detectedDisplay_.error : tr("Monitor profile disabled in Preferences"));
}
void Window::updateOverlays(const OverlayPatch& patch) {
    overlays_.apply(patch);
    if (open_) emit request(commands::Overlays{.overlays = overlays_});
}
void Window::updateState(const EditorState& state) {
    state_ = state;
    const auto detected = state.display;
    if (detected != detectedDisplay_) { detectedDisplay_ = detected; applyDisplayProfile(); }
    emit agent_->modelsFinished(state.modelDiscovery);
    observeExport(state.exporting);
    updateDuplicates(state.duplicates);
    if (open_ && !state.open) {histogram_->reset();adjustments_->setHistogram({});canvas_->setRetouchState(0,0,{});retouch_->reset();}
    open_ = state.open;
    adjustments_->setEnabled(open_);
    export_->setEnabled(open_ || !files_->selectedItems().isEmpty());
    exportButton_->setEnabled(open_ || !files_->selectedItems().isEmpty());
    undo_->setEnabled(state.canUndo);
    redo_->setEnabled(state.canRedo);
    agent_->setPhoto(open_ ? path_ : QString(), state.copy.has_value()
        ? QString::number(state.copy.value_or(0)) : QString(), state.photoIdentity);
    agent_->setState(state.agent, open_);
    const auto importing = state.importing;
    if (importing.running)
        statusBar()->showMessage(tr("Importing: %1 / %2 files")
                                     .arg(importing.files)
                                     .arg(importing.total));
    else if (importing.error.has_value())
        statusBar()->showMessage(tr("Import failed: %1").arg(*importing.error));
    if (!open_)
        return;
    if (state.extent) canvas_->setExtent(*state.extent);
    title_->setText(QFileInfo(path_).fileName() + " — Aigle");
    breadcrumb_->setText(tr("Pictures    /    %1    /    %2")
                             .arg(QFileInfo(folder_).fileName(), QFileInfo(path_).fileName()));
    setWindowTitle(QFileInfo(path_).fileName() + (state.dirty ? " *" : "") +
                   " — Aigle");
    const auto load = state.load;
    if (load.status == ImageLoad::Status::Loading)
        status_->setText(tr("Loading…"));
    else if (load.status == ImageLoad::Status::Failed)
        status_->setText(load.error);
    else {
        adjustments_->setImageInfo(load.info);
        const auto size = load.sourceSize;
        status_->setText(tr("%1 × %2 · %3")
                             .arg(size.width())
                             .arg(size.height())
                             .arg(state.dirty ? tr("Saving…") : tr("Saved")));
        const auto& camera = load.camera;
        QStringList details;
        const auto cameraName =
            (camera.make + ' ' + camera.model).trimmed();
        if (!cameraName.isEmpty())
            details.append(cameraName);
        if (!camera.lens.isEmpty())
            details.append(camera.lens);
        if (camera.iso.has_value())
            details.append(tr("ISO %1").arg(*camera.iso, 0, 'f', 0));
        if (camera.aperture.has_value())
            details.append(tr("f/%1").arg(*camera.aperture, 0, 'f', 1));
        if (camera.focalLength.has_value())
            details.append(tr("%1 mm").arg(*camera.focalLength, 0, 'f', 0));
        info_->setText(details.join('\n'));
        canvasInfo_->setText(details.join(" · "));
        dimensions_->setText(
            tr("%1 × %2").arg(size.width()).arg(size.height()));
        status_->setText(state.retouchPreview ? tr("Retouch preview · release to apply, Escape to cancel") : state.dirty ? tr("Saving…") : tr("All changes saved"));
    }
    if (state.params.has_value()) {
        params_ = *state.params;
        const auto document=state.document;
        const auto revision=state.revision;
        const auto retouchGuides=state.retouchGuides;
        canvas_->setRetouchState(document,revision,retouchGuides);
        retouch_->setState(state.retouch,document,revision,retouchGuides);
        toolsPanel_->setGuides(state.maskGuides);
        toolsPanel_->setParams(params_);
        canvas_->setEditParams(state.geometry);
        adjustments_->setParams(params_);
        const QSignalBlocker blocker(history_);
        history_->clear();
        history_->addItem(tr("Original"));
        for (const auto& entry : state.history) {
            const auto& item = entry;
            auto* row = new QListWidgetItem(item.label, history_);
            if (!item.applied)
                row->setForeground(palette().placeholderText());
        }
        history_->setCurrentRow(state.position);
        if (state.dirty)
            autosave_.start();
    }
    if (state.histogram.has_value()) {
        histogram_->histogramChanged(*state.histogram);
        adjustments_->setHistogram(*state.histogram);
    }
    {
        const QSignalBlocker blocker(rating_);
        rating_->setValue(state.rating.value_or(0));
    }
    if (!tags_->hasFocus()) {
        QStringList tags;
        for (const auto& tag : state.tags.value_or(QStringList{}))
            tags.append(tag);
        tags_->setText(tags.join(", "));
    }
    libraryInfo_->setText(info_->text());
    libraryInfo_->setVisible(!info_->text().isEmpty());
    if (auto* item = files_->currentItem(); item && samePhoto(item, photoIdentityFromJson(QJsonObject{{"path", path_}, {"copy", (state.copy ? QJsonValue(*state.copy) : QJsonValue(QJsonValue::Null))}}))) {
        auto meta = item->data(PhotoDelegate::MetadataRole).toJsonObject();
        const auto before = meta;
        if (state.rating) meta["rating"] = *state.rating;
        if (state.flag) meta["flag"] = *state.flag;
        if (state.label) meta["label"] = state.label->isNull() ? QJsonValue(QJsonValue::Null) : QJsonValue(*state.label);
        if (state.tags) meta["tags"] = QJsonArray::fromStringList(*state.tags);
        if (state.iptc) meta["iptc"] = toJson(*state.iptc);
        if (meta != before) {
            emit request(commands::LibraryMetadata{.path = path_, .copy = (state.copy ? QJsonValue(*state.copy) : QJsonValue(QJsonValue::Null))});
            item->setData(PhotoDelegate::MetadataRole, meta);
            item->setData(PhotoDelegate::RatingRole, state.rating.value_or(0));
        }
    }
    updateSelection();
    if (files_->selectedItems().size() <= 1) {
        const QSignalBlocker blocker(libraryRating_);
        libraryRating_->setValue(rating_->value());
        const auto stars =
            libraryMetadata_->findChild<QWidget*>("ratingStars")->findChildren<QToolButton*>();
        for (int i = 0; i < stars.size(); ++i)
            stars[i]->setText(i < rating_->value() ? QString::fromUtf8("★")
                                                   : QString::fromUtf8("☆"));
    }
    QStringList snapshotNames;
    for (const auto& name : state.snapshots)
        snapshotNames.append(name);
    QStringList existingSnapshots;
    for (int i = 0; i < snapshots_->count(); ++i)
        existingSnapshots.append(snapshots_->item(i)->text());
    if (snapshotNames != existingSnapshots) {
        snapshots_->clear();
        snapshots_->addItems(snapshotNames);
        snapshots_->setMinimumHeight(24);
    }
    if (state.dirty && !autosave_.isActive())
        autosave_.start();
    const auto exported = state.exporting;
    if (exported.running)
        status_->setText(tr("Exporting…"));
    if (exported.error.has_value())
        status_->setText(tr("Export failed: %1").arg(*exported.error));
    if (exported.hasResult)
        status_->setText(tr("Export complete"));
    if (smoke_) {
        const double exposure = valueAt(params_, "light.exposure").toDouble();
        if (smokeStep_ == 1 && exposure == 1) {
            smokeStep_ = 2;
            emit request(commands::Undo{});
        } else if (smokeStep_ == 2 && exposure == 0) {
            smokeStep_ = 3;
            emit request(commands::Redo{});
        } else if (smokeStep_ == 3 && exposure == 1) {
            smokeStep_ = 4;
            canvas_->setCompare(true);
            canvas_->fit();
        }
    }
}
void Window::showError(const QString& message) {
    status_->setText(message);
    closing_ = false;
    if (smoke_) {
        qCritical().noquote() << message;
        emit smokeFinished(2);
        return;
    }
    if (message == lastError_)
        return;
    lastError_ = message;
    QMessageBox::warning(this, tr("Aigle"), message);
}
void Window::exportPhoto() {
    if (!engineReady_ || exportPreparing_) return;
    auto command = commands::ExportInputs{.preview = true};
    if (developMode_ && files_->selectedItems().size() <= 1 && open_) command.current = true;
    else {
        QList<PhotoIdentity> targets;
        for (auto* item : files_->selectedItems()) targets.append(photoIdentity(item));
        if (!targets.isEmpty()) command.targets = targets;
        else if (open_) command.current = true;
        else return;
    }
    exportPreparing_ = true;
    emit request(command);
}
void Window::exportInputs(const QList<ExportInput>& inputs) {
    if (inputs.isEmpty()) return;
    const auto& preview = inputs.first().preview;
    const auto size =
        state_.load.sourceSize;
    ExportDialog dialog(inputs.first().path.isNull() ? path_ : inputs.first().path, preview, size, exportFormats_,
                        this, inputs);
    if (dialog.exec() != QDialog::Accepted)
        return;
    const auto jobs = dialog.requests();
    const bool hadPending = exportQueue_.hasPending();
    if (!exportQueue_.add(jobs)) { checkExportQueue(); return; }
    if (!hadPending) exportQueue_.paused = false;
    advanceExportQueue();

}
void Window::loadPreset() {
    if (!open_)
        return;
    const auto path =
        QFileDialog::getOpenFileName(this, tr("Load preset"), {}, tr("Aigle presets (*.json)"));
    if (path.isEmpty())
        return;
    applyPreset(path);
}
void Window::savePreset() {
    if (!open_)
        return;
    const auto directory =
        QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation) + "/aigle/presets";
    QDir().mkpath(directory);
    const auto path = QFileDialog::getSaveFileName(
        this, tr("Save preset"), directory + "/preset.json", tr("Aigle presets (*.json)"));
    if (path.isEmpty())
        return;
    QSaveFile file(path);
    const auto bytes = QJsonDocument(params_).toJson(QJsonDocument::Indented);
    if (!file.open(QIODevice::WriteOnly) || file.write(bytes) != bytes.size() || !file.commit())
        showError(file.errorString());
    else
        refreshPresets();
}
void Window::captureLayout(const QString& path, const QString& state) {
    qInfo() << "Layout capture" << size() << "minimum" << minimumSizeHint() << "dpr"
            << devicePixelRatioF();
    // Development capture uses the real Vulkan readback and real widget tree.
    // It never seeds edits, writes sidecars or substitutes sample UI data.
    autosave_.stop();
    setMode(!state.startsWith("library"));
    if (state.startsWith("scope-")) {
        leftOverride_ = 1;
        updateResponsiveLayout();
        const auto mode = state == "scope-vectorscope" ? 6 : state == "scope-parade" ? 5 : 4;
        histogram_->findChild<QComboBox*>("scopeMode")->setCurrentIndex(mode);
        histogram_->findChild<QCheckBox*>("scopeRegion")->setChecked(state == "scope-region");
    }
    if (state == "comparison") {
        // QWidget::grab cannot include native Vulkan child surfaces. Preserve
        // separate GPU readbacks alongside the dialog layout capture.
        QTimer::singleShot(9000, this, [this, path] {
            int index = 0;
            for (auto* window : QGuiApplication::allWindows())
                if (auto* canvas = qobject_cast<Canvas*>(window); canvas && canvas != canvas_ && canvas->isVisible())
                    canvas->capture(path + QString(".pane-%1.png").arg(index++));
        });
    }
    if (state == "library-filters" || state == "library-searches" || state == "export-jobs" || state == "comparison" || state == "copy-adjustments" || state == "preferences" || state == "shortcuts") {
        QTimer::singleShot(state == "comparison" ? 12000 : 200, this, [this, path] {
            if (auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget())) {
                auto screenshot = grab();
                QPainter painter(&screenshot);
                painter.fillRect(rect(), QColor(0, 0, 0, 128));
                painter.drawPixmap((width() - dialog->width()) / 2,
                                   (height() - dialog->height()) / 2, dialog->grab());
                painter.end();
                screenshot.toImage().scaled(size(), Qt::IgnoreAspectRatio, Qt::SmoothTransformation).save(path);
                dialog->reject();
                emit smokeFinished(0);
            } else emit smokeFinished(1);
        });
        if (state == "library-filters") editLibraryFilters();
        else if (state == "export-jobs") showExportQueue();
        else if (state == "copy-adjustments") transferAdjustments(false);
        else if (state == "preferences") editPreferences();
        else if (state == "shortcuts") editShortcuts();
        else if (state == "comparison") {
            QList<PhotoIdentity> paths;
            for (int i=0; i<std::min(4, files_->count()); ++i) paths.append(photoIdentity(files_->item(i)));
            showComparison(this, engine_, paths, [this](const PhotoIdentity& target, const QString& flag) {
                emit request(commands::BatchMetadata{.targets = QList<PhotoIdentity>{target}, .flag = flag});
            }, canvas_);
        }
        else manageSavedSearches();
        return;
    }
    if (state == "retouch" || state=="retouch-eye") {
        showInspector("heal");
        QTimer::singleShot(500,this,[this,state]{retouch_->select(state=="retouch-eye"?"eye":"spot",0);});
    }
    if (state == "crop-rotated" || state == "crop-rotated-zoom") {
        showInspector("crop");
        canvas_->setCropAngle(30);
        toolsPanel_->setCropAngle(30);
        if (state == "crop-rotated-zoom")
            QTimer::singleShot(600, this, [this] { canvas_->setNavigation({3, {.65, .4}}); });
    }
    if (state == "crop" || state == "masks" || state == "agent")
        showInspector(state);
    if (state == "library") {
        QTimer::singleShot(1200, this, [this, path] {
            grab()
                .toImage()
                .scaled(size(), Qt::IgnoreAspectRatio, Qt::SmoothTransformation)
                .save(path);
            emit smokeFinished(0);
        });
        return;
    }
    auto connection = std::make_shared<QMetaObject::Connection>();
    *connection = connect(
        canvas_, &Canvas::captured, this, [this, path, state, connection](const QImage& image) {
            disconnect(*connection);
            QFile::remove(path + ".canvas.png");
            auto screenshot = grab().toImage();
            QPainter painter(&screenshot);
            const auto position = canvas_->mapToGlobal(QPoint(0, 0)) - mapToGlobal(QPoint(0, 0));
            painter.drawImage(QRect(position, canvas_->size()), image);
            painter.end();
            if (state == "export-live") {
                auto* captureTimer = new QTimer(this);
                captureTimer->setInterval(100);
                connect(captureTimer, &QTimer::timeout, this, [this, path, screenshot, captureTimer]() mutable {
                    auto* dialog = qobject_cast<ExportDialog*>(QApplication::activeModalWidget());
                    if (!dialog || !dialog->isVisible()) return;
                    captureTimer->stop();
                    auto* preview = dialog->findChild<QLabel*>("photoPreview");
                    const bool hasPreview = preview && !preview->pixmap().isNull();
                    QPainter overlayPainter(&screenshot);
                    overlayPainter.fillRect(screenshot.rect(), QColor(0, 0, 0, 128));
                    overlayPainter.drawPixmap((width() - dialog->width()) / 2, (height() - dialog->height()) / 2, dialog->grab());
                    overlayPainter.end();
                    const bool saved = screenshot.scaled(size(), Qt::IgnoreAspectRatio, Qt::SmoothTransformation).save(path);
                    dialog->reject();
                    captureTimer->deleteLater();
                    emit smokeFinished(saved && hasPreview ? 0 : 2);
                });
                captureTimer->start();
                exportPhoto();
                return;
            }
            if (state == "export") {
                QImage preview;
                if (auto* item = files_->currentItem())
                    preview = qvariant_cast<QImage>(item->data(PhotoDelegate::ImageRole));
                const auto size = state_.load.sourceSize;
                auto* dialog = new ExportDialog(
                    path_, preview, size, exportFormats_, this);
                dialog->show();
                QTimer::singleShot(150, this, [this, path, screenshot, dialog]() mutable {
                    QPainter p(&screenshot);
                    p.fillRect(screenshot.rect(), QColor(0, 0, 0, 128));
                    p.drawPixmap((width() - dialog->width()) / 2, (height() - dialog->height()) / 2,
                                 dialog->grab());
                    p.end();
                    screenshot.scaled(this->size(), Qt::IgnoreAspectRatio, Qt::SmoothTransformation)
                        .save(path);
                    dialog->deleteLater();
                    emit smokeFinished(0);
                });
            } else {
                screenshot.scaled(this->size(), Qt::IgnoreAspectRatio, Qt::SmoothTransformation)
                    .save(path);
                emit smokeFinished(0);
            }
        });
    QTimer::singleShot(1200, this, [this, path] { canvas_->capture(path + ".canvas.png"); });
}
void Window::saveLayout() {
    settings_.setValue("gpuiLayout/geometry", saveGeometry());
}
void Window::closeEvent(QCloseEvent* event) {
    if (smoke_ || closeSaved_ || !open_) {
        saveLayout();
        event->accept();
        return;
    }
    event->ignore();
    if (closing_)
        return;
    closing_ = true;
    autosave_.stop();
    emit request(commands::Save{});
}
void Window::dragEnterEvent(QDragEnterEvent* event) {
    if (event->mimeData()->hasUrls())
        event->acceptProposedAction();
}
void Window::dropEvent(QDropEvent* event) {
    const auto urls = event->mimeData()->urls();
    if (urls.isEmpty() || !urls.first().isLocalFile())
        return;
    const auto path = urls.first().toLocalFile();
    if (QFileInfo(path).isDir())
        openFolder(path);
    else
        open(path);
    event->acceptProposedAction();
}
} // namespace aigle
