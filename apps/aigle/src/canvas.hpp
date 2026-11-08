#pragma once
#include "mask_types.hpp"
#include "retouch_types.hpp"
#include "commands.hpp"
#include "gui_types.hpp"
#include "engine.hpp"
#include "viewport.hpp"
#include "local_shortcuts.hpp"
#include <QJsonArray>
#include <QVector4D>
#include <QVulkanInstance>
#include <QWindow>
#include <array>
#include <memory>
#include <rhi/qrhi.h>

namespace aigle {
class Canvas final : public QWindow {
    Q_OBJECT
  public:
    explicit Canvas(QVulkanInstance* instance);
    ~Canvas() override;
    void setExtent(QSizeF extent);
    void setFrame(Frame frame, bool original);
    void setHdr(bool enabled);
    bool hdrRequested() const { return hdrRequested_; }
    void setDisplayProfile(DisplayProfile profile, QString name);
    void setCompare(bool enabled);
    void setSurveyMode(bool enabled) { surveyMode_ = enabled; }
    void setOriginal(bool enabled);
    void fit();
    [[nodiscard]] Viewport::Navigation navigation() const { return viewport_.navigation(); }
    void setNavigation(const Viewport::Navigation& value);
    void oneToOne();
    void toggleZoom();
    void capture(const QString& path);
    void setTool(const QString& tool);
    void setEditParams(const GeometryParameters& params);
    void setMaskGuide(const MaskGuide& guide);
    void setRetouchState(quint64 document, quint64 revision, const QList<RetouchGuide>& guides);
    void selectRetouch(const QString& kind, int index);
    void setRetouchDefaults(const RetouchDefaults& values);
    void setRetouchResult(const RetouchResult& result);
    void setCropAspect(double ratio);
    void setCropAngle(double angle);
    void beginCropAngleGesture();
    void endCropAngleGesture();
    void setCropGuide(int guide) {
        cropGuide_ = guide;
        requestUpdate();
    }
    void drawHorizon() {
        horizon_ = true;
        dragging_ = false;
    }
    void orientCrop(int operation);
    void applyCrop();
    void cancelCrop();
    void setRadius(double radius) { radius_ = radius; }
    [[nodiscard]] bool hdrActive() const noexcept { return encoding_ != 0; }
  signals:
    void viewportChanged(const aigle::ViewportRequest& request);
    void displayStatus(const QString& message);
    void rendered(bool hdr, quint64 revision);
    void captured(const QImage& image);
    void toolFinished();
    void surveyCloseRequested();
    void surveyFitRequested();
    void surveyPixelsRequested();
    void cropAngleUpdated(double angle);
    void toolCommand(const aigle::EngineCommand& command);
    void retouchSelected(const QString& kind, int index);
    void retouchError(const QString& error);

  private:
    DisplayProfile displayProfile_;
    QString displayName_ = "sRGB";
  protected:
    bool event(QEvent* event) override;
    void exposeEvent(QExposeEvent*) override;
    void resizeEvent(QResizeEvent*) override;
    void wheelEvent(QWheelEvent* event) override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent* event) override;
    void keyPressEvent(QKeyEvent* event) override;
    void mouseDoubleClickEvent(QMouseEvent*) override;

  private:
    bool surveyMode_ = false;
    LocalShortcuts shortcuts_;
    void deleteSelectedRetouch();
    void cancelDrag();
    commands::Retouch retouchRequest(commands::Retouch::Phase phase) const;
    RetouchGuide retouchGuide() const;
    void prepareRetouch(QRhiResourceUpdateBatch* updates);
    void drawRetouch(QRhiCommandBuffer* cb);
    void addRetouchLine(QVector<float>& vertices, QPointF from, QPointF to, QVector4D color,
                        double thickness = 1.2);
    bool initialize();
    void retryRender();
    void release();
    void render();
    void sendViewport(bool continuous = false);
    void configureOutput();
    [[nodiscard]] QVector4D mapping(const Frame& frame) const;
    QVulkanInstance* instance_;
    std::unique_ptr<QRhi> rhi_;
    std::unique_ptr<QRhiSwapChain> swapchain_;
    std::unique_ptr<QRhiRenderPassDescriptor> pass_;
    std::unique_ptr<QRhiGraphicsPipeline> pipeline_;
    std::unique_ptr<QRhiShaderResourceBindings> bindings_;
    std::unique_ptr<QRhiBuffer> uniform_;
    std::unique_ptr<QRhiSampler> sampler_;
    std::array<std::unique_ptr<QRhiTexture>, 2> textures_;
    std::array<Frame, 2> frames_;
    std::array<bool, 2> upload_{true, true};
    Viewport viewport_;
    QPointF lastMouse_;
    bool dragging_ = false, hdrRequested_ = false, compare_ = false, original_ = false, configured_ = false;
    int encoding_ = 0;
    QString capturePath_;
    QRhiReadbackResult readback_;
    QString tool_ = "pan";
    double radius_ = 0.02;
    QPointF dragStart_;
    QList<QPointF> toolPoints_;
    QTimer settle_;
    QTimer renderRetry_;
    int renderRetryMs_ = 250;
    QRectF constrainedCrop(const QRectF& start, int handle = -1, QPointF delta = {}, bool refitAspect = false) const;
    bool cropAngleGesture_ = false;
    QRectF cropAngleAnchor_;
    GeometryParameters editParams_;
    MaskGuide maskGuide_, maskStart_;
    QList<RetouchGuide> retouchGuides_;
    RetouchGuide retouchStart_, retouchPreview_;
    std::optional<commands::Retouch> retouchPending_;
    RetouchDefaults retouchDefaults_;
    QString retouchKind_, retouchHandle_, retouchSession_;
    int retouchIndex_ = -1, retouchVertexCount_ = 0, retouchCapacity_ = 0;
    quint64 retouchDocument_ = 0, retouchRevision_ = 0;
    QTimer retouchTimer_;
    std::unique_ptr<QRhiBuffer> retouchVertices_, retouchUniform_;
    std::unique_ptr<QRhiShaderResourceBindings> retouchBindings_;
    std::unique_ptr<QRhiGraphicsPipeline> retouchPipeline_;
    QRectF cropDraft_{0, 0, 1, 1}, cropStart_;
    QPointF cropPivot_;
    double cropPointerAngle_ = 0, cropAngleStart_ = 0;
    Orientation cropOrientation_;
    CropAspect cropAspectValue_;
    double cropAspect_ = -1, cropAngle_ = 0;
    int dragPart_ = 0, cropGuide_ = 0;
    bool horizon_ = false;
    QPointF imagePoint(QPointF point) const;
    QPointF windowPoint(QPointF point) const;
};
} // namespace aigle
