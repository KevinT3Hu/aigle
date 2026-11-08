#pragma once
#include "replies.hpp"
#include "export_input.hpp"
#include "editor_state.hpp"
#include "library_types.hpp"
#include "commands.hpp"
#include "engine.hpp"
#include "preferences.hpp"
#include "export_queue.hpp"
#include <QAction>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMainWindow>
#include <QSettings>
#include <QSpinBox>
#include <QThread>
#include <QTimer>

class QVulkanInstance;
class QCloseEvent;
class QComboBox;
class QMenuBar;
class QStackedWidget;
class QToolButton;
class QResizeEvent;
class QTabWidget;
class QSplitter;
class QProgressDialog;
namespace aigle {
class Canvas;
class Adjustments;
class Scopes;
class AgentPanel;
class ToolPanel;
class RetouchPanel;
class PhotoDelegate;
class SelectionMetadata;
class Window final : public QMainWindow {
    Q_OBJECT
  public:
    explicit Window(QVulkanInstance* instance, QString initial, bool synthetic, bool smoke = false,
                    std::optional<EngineCommand> initialCommand = {});
    ~Window() override;
    void setHdr(bool enabled);
    void captureLayout(const QString& path, const QString& state);
  signals:
    void request(const aigle::EngineCommand& request);
    void viewportRequested(const aigle::ViewportRequest& request);
    void smokeFinished(int status);

  protected:
    void resizeEvent(QResizeEvent* event) override;
    bool eventFilter(QObject* watched, QEvent* event) override;
    void closeEvent(QCloseEvent* event) override;
    void dragEnterEvent(QDragEnterEvent* event) override;
    void dropEvent(QDropEvent* event) override;

  private:
    void open(const QString& path, bool develop = true, qint64 copy = 0);
    void openFolder(const QString& path);
    void receive(const EngineReply& reply);
    void detectDisplay();
    void applyDisplayProfile();
    DisplayDetection detectedDisplay_;
    void updateOverlays(const OverlayPatch& patch);
    OverlayPatch overlays_{.clipping = false, .focusPeaking = false, .mask = std::optional<qint64>{}};
    void updateState(const EditorState& state);
    void showError(const QString& message);
    void buildMenus();
    void initializeShortcuts();
    void reloadShortcuts();
    void editShortcuts();
    void editPreferences();
    Preferences preferences_;
    bool shortcutOverride(QObject* watched, QEvent* event);
    QList<QAction*> shortcutActions_;
    void buildWorkspace();
    void setMode(bool develop);
    void selectTool(const QString& tool);
    void refreshPresets();
    void applyPreset(const QString& path);
    void exportPhoto();
    void exportInputs(const QList<ExportInput>& inputs);
    void loadPreset();
    void savePreset();
    void saveLayout();
    void updateResponsiveLayout();
    void updatePhotoLayout();
    void updateSelection();
    void translateLibraryLabels();
    void translateDocumentStatus();
    void batchMetadata(commands::Metadata fields);
    void transferAdjustments(bool sync);
    void filterPhotos();
    void manageSavedSearches();
    qint64 activeAlbumId_ = 0;
    QString activeAlbumName_;
    void editLibraryFilters();
    void sortPhotos();
    void stackSelection(bool remove);
    void findDuplicates();
    void updateDuplicates(const DuplicateResult& state);
    void startFileActions(const QString& action);
    void nextFileAction();
    void finishFileAction(const FileActionResult& result);
    bool collapseStacks_ = false;
    qint64 duplicateId_ = 0;
    QProgressDialog* duplicateProgress_ = nullptr;
    QProgressDialog* fileProgress_ = nullptr;
    QQueue<commands::FileAction> fileActions_;
    QStringList fileErrors_;
    int fileCompleted_ = 0;

    LibraryFilter libraryFilter_;
    void showInspector(const QString& page);
    QWidget *leftRail_ = nullptr, *rightRail_ = nullptr, *sources_ = nullptr;
    QWidget* canvasSurround_ = nullptr;
    QLabel *breadcrumb_ = nullptr, *selection_ = nullptr, *canvasInfo_ = nullptr,
           *dimensions_ = nullptr, *filmCount_ = nullptr, *gridCount_ = nullptr,
           *libraryPreview_ = nullptr, *librarySelection_ = nullptr;
    QToolButton *exportButton_ = nullptr, *compareButton_ = nullptr, *zoomButton_ = nullptr;
    QTabWidget* references_ = nullptr;
    PhotoDelegate* photoDelegate_ = nullptr;
    RetouchPanel* retouch_;
    ToolPanel* toolsPanel_ = nullptr;
    QList<QToolButton*> canvasTools_;
    int leftOverride_ = -1, filmOverride_ = -1, layoutClass_ = -1;
    QString inspectorPage_ = "edit";
    QList<ExportFormat> exportFormats_;
    ExportQueue exportQueue_;
    bool engineReady_ = false;
    bool exportPreparing_ = false;
    bool queueErrorReported_ = false;
    void showExportQueue();
    void advanceExportQueue();
    void cancelExports();
    void observeExport(const ExportProgress& state);
    bool checkExportQueue();
    Engine* engine_;
    QThread thread_;
    Canvas* canvas_;
    Adjustments* adjustments_;
    Scopes* histogram_;
    AgentPanel* agent_;
    QListWidget *files_, *history_;
    QLabel *status_, *display_, *info_;
    QLineEdit *tags_, *filter_;
    QSpinBox* rating_;
    QAction *undo_ = nullptr, *redo_ = nullptr, *export_ = nullptr, *hdr_ = nullptr;
    QComboBox* tool_;
    QMenuBar* appMenus_;
    QStackedWidget *workspace_, *rightPages_, *developContent_;
    QWidget *gridHolder_, *filmstrip_, *historyPane_, *libraryMetadata_;
    QToolButton *gridButton_, *developButton_, *queueButton_;
    QAction *gridAction_ = nullptr, *developAction_ = nullptr;
    QLabel *title_, *folderTitle_, *libraryInfo_, *emptyLibrary_;
    QListWidget *snapshots_, *presets_;
    QSpinBox* libraryRating_;
    SelectionMetadata* selectionMetadata_ = nullptr;
    bool developMode_ = false;
    QJsonObject params_;
    EditorState state_;
    std::optional<ViewportRequest> pendingViewport_;
    QJsonObject metadataSchema_;
    QTimer viewportTimer_, autosave_, libraryViewTimer_;
    QSettings settings_;
    QString path_, pendingPath_, folder_, lastError_;
    bool open_ = false, closing_ = false, closeSaved_ = false, smoke_ = false;
    int smokeStep_ = 0;
};
} // namespace aigle
