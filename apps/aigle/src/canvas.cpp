#include "geometry_json.hpp"
#include "retouch_json.hpp"
#include "commands.hpp"
#include "gui_json.hpp"
#include "canvas.hpp"
#include <numeric>
#include <numbers>
#include "adjustments.hpp"
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QPlatformSurfaceEvent>
#include <QPolygonF>
#include <QUuid>
#include <QWheelEvent>
#include <cmath>
#include <cstring>
#include <limits>
#include <rhi/qrhi_platform.h>
#include <utility>

namespace aigle {
namespace {
QShader shader(const QString& name) {
    QFile file(":/shaders/" + name + ".qsb");
    return file.open(QIODevice::ReadOnly) ? QShader::fromSerialized(file.readAll()) : QShader{};
}
} // namespace
Canvas::Canvas(QVulkanInstance* instance) : instance_(instance) {
    retouchTimer_.setSingleShot(true);
    retouchTimer_.setInterval(40);
    connect(&retouchTimer_, &QTimer::timeout, this, [this] {
        if (dragging_ && retouchPending_.has_value()) {
            emit toolCommand(*retouchPending_);
            retouchPending_ = {};
        }
    });
    renderRetry_.setSingleShot(true);
    connect(&renderRetry_, &QTimer::timeout, this, [this] {
        if (isExposed()) requestUpdate();
    });
    settle_.setSingleShot(true);
    settle_.setInterval(120);
    connect(&settle_, &QTimer::timeout, this, [this] { sendViewport(); });
    setSurfaceType(QSurface::VulkanSurface);
    setVulkanInstance(instance);
    connect(this, &QWindow::screenChanged, this, [this] {
        configured_ = false;
        requestUpdate();
    });
}
Canvas::~Canvas() {
    release();
}
void Canvas::setExtent(QSizeF extent) {
    if (extent.width() <= 0 || extent.height() <= 0 || viewport_.image == extent)
        return;
    cancelDrag();
    viewport_.image = extent;
    if (tool_ == "crop" && cropAspect_ > 0) {
        // An extent update is not a user request to maximize the crop.
        const double normalized = cropAspect_ * extent.height() / extent.width();
        double w = cropDraft_.width(), h = w / normalized;
        if (h > cropDraft_.height()) {
            h = cropDraft_.height();
            w = h * normalized;
        }
        cropDraft_ = QRectF(cropDraft_.center() - QPointF(w / 2, h / 2), QSizeF(w, h));
        cropDraft_ = constrainedCrop(cropDraft_);
    }
    sendViewport();
}
void Canvas::setFrame(Frame frame, bool original) {
    const auto i = original ? 1U : 0U;
    const auto pixels = aigle_frame_pixels(frame.get());
    // Old requests may finish after switching the output mode. Never interpret
    // SDR bytes as HDR (or HDR bytes as SDR).
    if (frame && (pixels.format == 1) != hdrActive())
        return;
    frames_[i] = std::move(frame);
    upload_[i] = true;
    requestUpdate();
}
void Canvas::setHdr(bool enabled) {
    hdrRequested_ = enabled;
    configured_ = false;
    requestUpdate();
}
void Canvas::setCompare(bool enabled) {
    compare_ = enabled;
    requestUpdate();
}
void Canvas::setOriginal(bool enabled) {
    original_ = enabled;
    requestUpdate();
}
void Canvas::setNavigation(const Viewport::Navigation& value) {
    cancelDrag();
    viewport_.output = QSizeF(std::max(1, width()), std::max(1, height())) * devicePixelRatio();
    viewport_.setNavigation(value);
    sendViewport();
    requestUpdate();
}
void Canvas::fit() {
    cancelDrag();
    viewport_.fit();
    sendViewport();
    requestUpdate();
}
void Canvas::toggleZoom() {
    if (viewport_.scale() > viewport_.fitScale() * 1.0001) fit();
    else oneToOne();
}
void Canvas::oneToOne() {
    cancelDrag();
    viewport_.zoom(1, QPointF(width(), height()) * devicePixelRatio() / 2);
    sendViewport();
    requestUpdate();
}
void Canvas::capture(const QString& path) {
    qInfo() << "Capture canvas" << size() << "exposed" << isExposed() << "visible" << isVisible()
            << "configured" << configured_ << "frame" << bool(frames_[0]);
    capturePath_ = path;
    requestUpdate();
}
void Canvas::setTool(const QString& tool) {
    endCropAngleGesture();
    shortcuts_.reset();
    cancelDrag();
    if (tool == "crop" && tool_ != "crop") {
        cropDraft_ = editParams_.crop;
        cropOrientation_ = editParams_.orientation;
        cropAspectValue_ = editParams_.aspect;
        const auto& aspect = cropAspectValue_;
        cropAspect_ = aspect.kind == CropAspect::Kind::Original ? -1 : 0;
        if (aspect.kind == CropAspect::Kind::Ratio && aspect.height > 0)
            cropAspect_ = aspect.width / aspect.height;
        cropAngle_ = editParams_.angle;
    }
    horizon_ = false;
    tool_ = tool;
    dragging_ = false;
    setCursor(tool_ == "pan" ? Qt::OpenHandCursor : Qt::CrossCursor);
    requestUpdate();
}
QPointF Canvas::imagePoint(QPointF point) const {
    return viewport_.region().topLeft() + point * devicePixelRatio() / viewport_.scale();
}
QPointF Canvas::windowPoint(QPointF point) const {
    return (point - viewport_.region().topLeft()) * viewport_.scale() / devicePixelRatio();
}
void Canvas::setEditParams(const GeometryParameters& params) {
    editParams_ = params;
}
void Canvas::setMaskGuide(const MaskGuide& guide) {
    if (dragging_ && guide.valid && guide.id != maskGuide_.id)
        cancelDrag();
    if (!dragging_) {
        maskGuide_ = guide;
        requestUpdate();
    }
}
void Canvas::setRetouchState(quint64 document, quint64 revision, const QList<RetouchGuide>& guides) {
    if (document != retouchDocument_ || revision != retouchRevision_) {
        cancelDrag();
        retouchPreview_ = {};
    }
    if (document != retouchDocument_) {
        retouchKind_.clear();
        retouchIndex_ = -1;
    }
    retouchDocument_ = document;
    retouchRevision_ = revision;
    retouchGuides_ = guides;
    requestUpdate();
}
void Canvas::selectRetouch(const QString& kind, int index) {
    if (kind == retouchKind_ && index == retouchIndex_)
        return;
    cancelDrag();
    retouchKind_ = kind;
    retouchIndex_ = index;
    retouchPreview_ = {};
    requestUpdate();
}
void Canvas::setRetouchDefaults(const RetouchDefaults& values) {
    retouchDefaults_ = values;
    radius_ = values.radius;
    requestUpdate();
}
RetouchGuide Canvas::retouchGuide() const {
    if (retouchPreview_.valid)
        return retouchPreview_;
    for (const auto& value : retouchGuides_) {
        const auto& guide = value;
        if (guide.kind == retouchKind_ && guide.index == retouchIndex_)
            return guide;
    }
    return {};
}
commands::Retouch Canvas::retouchRequest(commands::Retouch::Phase phase) const {
    const auto from = imagePoint(dragStart_), to = imagePoint(lastMouse_);
    return commands::Retouch{.phase = phase, .action = "transform",
        .document_id = qint64(retouchDocument_), .revision = qint64(retouchRevision_),
        .session = retouchSession_, .kind = retouchKind_, .index = retouchIndex_,
        .expected = retouchStart_.value, .handle = retouchHandle_,
        .points = QList<QPointF>{from, to}};
}
void Canvas::setRetouchResult(const RetouchResult& result) {
    if (result.session != retouchSession_)
        return;
    if (result.error.has_value()) {
        cancelDrag();
        retouchPreview_ = {};
        emit retouchError(*result.error);
        return;
    }
    if (dragging_ && result.guide.has_value()) {
        retouchPreview_ = *result.guide;
        retouchPreview_.placement = result.placement;
        requestUpdate();
    }
}
QRectF Canvas::constrainedCrop(const QRectF& start, int handle, QPointF delta, bool refitAspect) const {
    auto geometry = editParams_;
    geometry.angle = cropAngle_;
    geometry.aspect = cropAspectValue_;
    const QJsonObject request{{"action", refitAspect ? "aspect" : handle < 0 ? "fit" : "drag"},
        {"width", qRound(viewport_.image.width())}, {"height", qRound(viewport_.image.height())},
        {"geometry", toJson(geometry)}, {"handle", handle}, {"dx", delta.x()}, {"dy", delta.y()},
        {"crop", QJsonObject{{"x", start.x()}, {"y", start.y()},
                             {"w", start.width()}, {"h", start.height()}}}};
    const auto bytes = QJsonDocument(request).toJson(QJsonDocument::Compact);
    char* response = aigle_crop_geometry(bytes.constData());
    const auto result = QJsonDocument::fromJson(response).object();
    aigle_string_free(response);
    if (!result.contains("ok")) return start;
    const auto crop = result["ok"].toObject();
    return {crop["x"].toDouble(), crop["y"].toDouble(),
            crop["w"].toDouble(), crop["h"].toDouble()};
}
void Canvas::beginCropAngleGesture() {
    cropAngleAnchor_ = cropDraft_;
    cropAngleGesture_ = true;
}
void Canvas::endCropAngleGesture() {
    cropAngleGesture_ = false;
}
void Canvas::setCropAngle(double angle) {
    cropAngle_ = angle;
    cropDraft_ = constrainedCrop(cropAngleGesture_ ? cropAngleAnchor_ : cropDraft_);
    sendViewport(true);
    settle_.start();
    requestUpdate();
}
void Canvas::setCropAspect(double ratio) {
    if (!std::isfinite(ratio)) return;
    cropAspect_ = ratio;
    cropAspectValue_ = {ratio < 0 ? CropAspect::Kind::Original : CropAspect::Kind::Free};
    if (ratio > 0) {
        // Find a compact rational for presets and custom integer ratios.
        int denominator = 1;
        while (denominator < 100000 && std::abs(ratio * denominator - std::round(ratio * denominator)) > 1e-7)
            ++denominator;
        const int numerator = qRound(std::min(ratio * denominator, double(INT_MAX)));
        const int divisor = std::gcd(std::max(1, numerator), denominator);
        cropAspectValue_ = {CropAspect::Kind::Ratio, double(std::max(1, numerator) / divisor), double(denominator / divisor)};
    }
    if (ratio != 0) cropDraft_ = constrainedCrop(cropDraft_, -1, {}, true);
    requestUpdate();
}
void Canvas::orientCrop(int operation) {
    if (tool_ != "crop")
        return;
    int turns = cropOrientation_.quarterTurns;
    bool flip = cropOrientation_.flip;
    const auto r = cropDraft_;
    if (operation == 0 || operation == 1) {
        const bool clockwise = operation == 1;
        turns = (turns + (clockwise ? 1 : 3)) % 4;
        cropDraft_ = clockwise ? QRectF(1 - r.bottom(), r.left(), r.height(), r.width())
                               : QRectF(r.top(), 1 - r.right(), r.height(), r.width());
    } else {
        turns = ((operation == 2 ? 4 : 6) - turns) % 4;
        flip = !flip;
        cropDraft_ = operation == 2 ? QRectF(1 - r.right(), r.top(), r.width(), r.height())
                                    : QRectF(r.left(), 1 - r.bottom(), r.width(), r.height());
        cropAngle_ = -cropAngle_;
        emit cropAngleUpdated(cropAngle_);
    }
    cropOrientation_ = {turns, flip};
    horizon_ = dragging_ = false;
    viewport_.fit();
    emit toolCommand(commands::CropOrientation{.orientation = cropOrientation_});
    requestUpdate();
}
void Canvas::applyCrop() {
    if (tool_ != "crop")
        return;
    const auto a = cropDraft_.topLeft(), b = cropDraft_.bottomRight();
    emit toolCommand(commands::Tool{.tool = "crop", .angle = cropAngle_, .aspect = cropAspectValue_, .orientation = cropOrientation_, .normalized = true, .points = QList<QPointF>{a, b}});
    emit toolFinished();
}
void Canvas::cancelCrop() {
    dragging_ = false;
    requestUpdate();
    emit toolFinished();
}
void Canvas::deleteSelectedRetouch() {
    if (tool_ != "retouch" || retouchIndex_ < 0) return;
    cancelDrag();
    const auto guide = retouchGuide();
    retouchSession_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
    emit toolCommand(commands::Retouch{.action = "delete", .document_id = qint64(retouchDocument_), .revision = qint64(retouchRevision_), .session = retouchSession_, .kind = retouchKind_, .index = retouchIndex_, .expected = guide.value, .label = tr("Delete correction")});
}
void Canvas::keyPressEvent(QKeyEvent* event) {
    if (surveyMode_ && event->modifiers() == Qt::NoModifier) {
        const auto center = QPointF(width(), height()) * devicePixelRatio() / 2;
        switch (event->key()) {
        case Qt::Key_Plus: case Qt::Key_Equal: viewport_.zoom(viewport_.scale() * 1.2, center); break;
        case Qt::Key_Minus: viewport_.zoom(viewport_.scale() / 1.2, center); break;
        case Qt::Key_Left: viewport_.pan({40 * devicePixelRatio(), 0}); break;
        case Qt::Key_Right: viewport_.pan({-40 * devicePixelRatio(), 0}); break;
        case Qt::Key_Up: viewport_.pan({0, 40 * devicePixelRatio()}); break;
        case Qt::Key_Down: viewport_.pan({0, -40 * devicePixelRatio()}); break;
        default: QWindow::keyPressEvent(event); return;
        }
        sendViewport();
        requestUpdate();
        event->accept();
        return;
    }
    QWindow::keyPressEvent(event);
}
bool Canvas::initialize() {
    if (rhi_)
        return true;
    QRhiVulkanInitParams params;
    params.inst = instance_;
    params.window = this;
    rhi_.reset(QRhi::create(QRhi::Vulkan, &params));
    if (!rhi_) {
        emit displayStatus(tr("Cannot create Vulkan renderer"));
        return false;
    }
    swapchain_.reset(rhi_->newSwapChain());
    swapchain_->setWindow(this);
    swapchain_->setFlags(QRhiSwapChain::UsedAsTransferSource);
    uniform_.reset(rhi_->newBuffer(QRhiBuffer::Dynamic, QRhiBuffer::UniformBuffer, 1120));
    sampler_.reset(rhi_->newSampler(QRhiSampler::Linear, QRhiSampler::Linear, QRhiSampler::None,
                                    QRhiSampler::ClampToEdge, QRhiSampler::ClampToEdge));
    if (!uniform_->create() || !sampler_->create()) {
        release();
        emit displayStatus(tr("Cannot create canvas resources"));
        return false;
    }
    return true;
}
void Canvas::setDisplayProfile(DisplayProfile profile, QString name) {
    if (displayProfile_ == profile && displayName_ == name) return;
    displayProfile_ = profile;
    displayName_ = name;
    if (!hdrActive()) {
        frames_ = {};
        textures_ = {};
        upload_ = {true, true};
        emit displayStatus(tr("SDR · %1").arg(displayName_));
        sendViewport();
        requestUpdate();
    }
}
void Canvas::configureOutput() {
    retouchPipeline_.reset();
    retouchBindings_.reset();
    retouchVertices_.reset();
    retouchUniform_.reset();
    retouchCapacity_ = retouchVertexCount_ = 0;
    pipeline_.reset();
    bindings_.reset();
    pass_.reset();
    swapchain_->destroy();
    int next = 0;
    auto format = QRhiSwapChain::SDR;
    if (hdrRequested_) {
        if (swapchain_->isFormatSupported(QRhiSwapChain::HDR10)) {
            format = QRhiSwapChain::HDR10;
            next = 1;
        } else if (swapchain_->isFormatSupported(QRhiSwapChain::HDRExtendedSrgbLinear)) {
            format = QRhiSwapChain::HDRExtendedSrgbLinear;
            next = 2;
        }
    }
    swapchain_->setFormat(format);
    pass_.reset(swapchain_->newCompatibleRenderPassDescriptor());
    swapchain_->setRenderPassDescriptor(pass_.get());
    if (!swapchain_->createOrResize()) {
        configured_ = false;
        emit displayStatus(tr("Cannot create display swapchain"));
        return;
    }
    if (next != encoding_) {
        frames_ = {};
        textures_ = {};
        upload_ = {true, true};
    }
    encoding_ = next;
    configured_ = true;
    emit displayStatus(next == 1       ? tr("HDR · Rec.2020 / PQ")
                       : next == 2     ? tr("HDR · linear scRGB")
                       : hdrRequested_ ? tr("HDR unavailable on this output · SDR")
                                       : tr("SDR · %1").arg(displayName_));
    sendViewport();
}
void Canvas::release() {
    retouchPipeline_.reset();
    retouchBindings_.reset();
    retouchVertices_.reset();
    retouchUniform_.reset();
    retouchCapacity_ = retouchVertexCount_ = 0;
    if (swapchain_)
        swapchain_->destroy();
    pipeline_.reset();
    bindings_.reset();
    textures_ = {};
    sampler_.reset();
    uniform_.reset();
    pass_.reset();
    swapchain_.reset();
    rhi_.reset();
    configured_ = false;
}
void Canvas::cancelDrag() {
    if (!dragging_)
        return;
    dragging_ = false;
    if (tool_ == "retouch") {
        retouchTimer_.stop();
        retouchPending_ = {};
        if (!retouchHandle_.isEmpty())
            emit toolCommand(commands::RetouchCancel{.session = retouchSession_});
        retouchHandle_.clear();
        retouchSession_.clear();
        retouchPreview_ = {};
    }
    if (tool_ == "mask_edit")
        maskGuide_ = maskStart_;
    if (tool_ == "crop") {
        cropDraft_ = cropStart_;
        cropAngle_ = cropAngleStart_;
        emit cropAngleUpdated(cropAngle_);
    }
    if (tool_ == "pan")
        setCursor(Qt::OpenHandCursor);
    toolPoints_ = {};
    horizon_ = false;
    requestUpdate();
}
bool Canvas::event(QEvent* event) {
    if (surveyMode_) {
        const QList<LocalShortcuts::Binding> surveyBindings{
            {"comparison_fit", "F"}, {"comparison_pixels", "1"}, {"dialog_cancel", "Esc"}};
        if (shortcuts_.handle(event, surveyBindings, [this](const QString& id) {
            if (id == "comparison_fit") emit surveyFitRequested();
            else if (id == "comparison_pixels") emit surveyPixelsRequested();
            else emit surveyCloseRequested();
        })) return true;
    }
    QList<LocalShortcuts::Binding> bindings;
    if (!surveyMode_) bindings.append(LocalShortcuts::Binding{"tool_cancel", "Esc"});
    if (tool_ == "crop") {
        bindings.append(LocalShortcuts::Binding{"crop_accept", "Return"});
        bindings.append(LocalShortcuts::Binding{"crop_swap_aspect", "X"});
    }
    if (tool_ == "retouch" && retouchIndex_ >= 0) {
        bindings.append(LocalShortcuts::Binding{"retouch_delete", "Delete"});
        bindings.append(LocalShortcuts::Binding{"retouch_delete_alternate", "Backspace"});
    }
    if (shortcuts_.handle(event, bindings, [this](const QString& id) {
        if (id == "tool_cancel") {
            if (dragging_) cancelDrag(); else cancelCrop();
        } else if (id == "crop_accept") applyCrop();
        else if (id == "crop_swap_aspect") {
            const double ratio = cropAspect_ < 0 ? viewport_.image.width() / viewport_.image.height() : cropAspect_;
            if (ratio > 0) setCropAspect(1.0 / ratio);
        } else deleteSelectedRetouch();
    })) return true;
    if (event->type() == QEvent::FocusOut || event->type() == QEvent::UngrabMouse ||
        event->type() == QEvent::WindowDeactivate)
        cancelDrag();
    if (event->type() == QEvent::UpdateRequest) {
        render();
        return true;
    }
    if (event->type() == QEvent::PlatformSurface &&
        static_cast<QPlatformSurfaceEvent*>(event)->surfaceEventType() ==
            QPlatformSurfaceEvent::SurfaceAboutToBeDestroyed)
        release();
    if (event->type() == QEvent::DevicePixelRatioChange)
        sendViewport();
    return QWindow::event(event);
}
void Canvas::exposeEvent(QExposeEvent*) {
    if (isExposed())
        render();
}
void Canvas::resizeEvent(QResizeEvent*) {
    if (tool_ == "retouch")
        cancelDrag();
    sendViewport();
    requestUpdate();
}
void Canvas::sendViewport(bool continuous) {
    viewport_.output = QSizeF(std::max(1, width()), std::max(1, height())) * devicePixelRatio();
    const double rotation = tool_ == "crop"
        ? cropAngle_ - editParams_.angle : 0;
    const auto r = viewport_.rotationSamplingRegion(rotation);
    auto output = r.size() * viewport_.scale();
    const double limit = std::min(1., 16384. / std::max(output.width(), output.height()));
    output *= limit;
    emit viewportChanged({r,
        {std::max(1, qRound(output.width())), std::max(1, qRound(output.height()))},
        hdrActive() ? DisplayProfile{DisplayProfile::Kind::HdrRec2020, {}} : displayProfile_,
        continuous});
}
QVector4D Canvas::mapping(const Frame& frame) const {
    if (!frame)
        return {2, 2, 0, 0};
    const auto p = aigle_frame_pixels(frame.get());
    const auto r = viewport_.region();
    return {float((r.x() - p.x) / p.w), float((r.y() - p.y) / p.h), float(r.width() / p.w),
            float(r.height() / p.h)};
}
void Canvas::addRetouchLine(QVector<float>& vertices, QPointF from, QPointF to, QVector4D color,
                            double thickness) {
    const auto delta = to - from;
    const auto length = std::hypot(delta.x(), delta.y());
    if (length < 0.00001)
        return;
    const QPointF n(-delta.y() / length * thickness / 2, delta.x() / length * thickness / 2);
    for (const auto p : {from + n, from - n, to + n, to + n, from - n, to - n}) {
        vertices.append(float(p.x()));
        vertices.append(float(p.y()));
        vertices.append(color.x());
        vertices.append(color.y());
        vertices.append(color.z());
        vertices.append(color.w());
    }
}
void Canvas::prepareRetouch(QRhiResourceUpdateBatch* updates) {
    retouchVertexCount_ = 0;
    if (tool_ != "retouch" || (retouchGuides_.isEmpty() && !retouchPreview_.valid))
        return;
    QVector<float> vertices;
    const auto point = [this](QPointF value) { return windowPoint(value); };
    const auto line = [&](const QList<QPointF>& outline, QVector4D color) {
        for (qsizetype i = 1; i < outline.size(); ++i)
            addRetouchLine(vertices, point(outline[i - 1]), point(outline[i]), color);
    };
    const auto dot = [&](QPointF center, double radius, QVector4D color) {
        QPointF previous = center + QPointF(radius, 0);
        for (int i = 1; i <= 16; ++i) {
            const double angle = i * 2 * 3.141592653589793 / 16;
            const auto next = center + QPointF(std::cos(angle) * radius, std::sin(angle) * radius);
            addRetouchLine(vertices, previous, next, color, 1.5);
            previous = next;
        }
    };
    const QVector4D idle(hdrActive() ? 0.38F : 0.68F, hdrActive() ? 0.38F : 0.68F,
                         hdrActive() ? 0.38F : 0.68F, 1);
    const QVector4D selected(hdrActive() ? 0.5F : 0.8F, hdrActive() ? 0.56F : 0.9F,
                             hdrActive() ? 0.6F : 1.F, 1);
    const QVector4D source(hdrActive() ? 0.6F : 1.F, hdrActive() ? 0.55F : 0.85F,
                           hdrActive() ? 0.4F : 0.5F, 1);
    auto visible = retouchGuides_;
    if (retouchPreview_.placement)
        visible.append(retouchPreview_);
    for (const auto& value : visible) {
        auto guide = value;
        const bool active =
            guide.placement ||
            (guide.kind == retouchKind_ && guide.index == retouchIndex_);
        if (active && retouchPreview_.valid)
            guide = retouchPreview_;
        line(guide.outline, active ? selected : idle);
        if (active) {
            dot(point(guide.target), 4, selected);
            dot(point(guide.radiusHandle), 5, selected);
            if (guide.source.has_value()) {
                line(guide.sourceOutline, source);
                dot(point(*guide.source), 4, source);
                addRetouchLine(vertices, point(guide.target), point(*guide.source), source,
                               0.8);
            }
        }
    }
    if (vertices.isEmpty())
        return;
    if (!retouchPipeline_) {
        retouchUniform_.reset(rhi_->newBuffer(QRhiBuffer::Dynamic, QRhiBuffer::UniformBuffer, 16));
        if (!retouchUniform_->create())
            return;
        retouchBindings_.reset(rhi_->newShaderResourceBindings());
        retouchBindings_->setBindings({QRhiShaderResourceBinding::uniformBuffer(
            0, QRhiShaderResourceBinding::VertexStage, retouchUniform_.get())});
        if (!retouchBindings_->create())
            return;
        auto pipeline = std::unique_ptr<QRhiGraphicsPipeline>(rhi_->newGraphicsPipeline());
        pipeline->setShaderStages({{QRhiShaderStage::Vertex, shader("retouch.vert")},
                                   {QRhiShaderStage::Fragment, shader("retouch.frag")}});
        QRhiVertexInputLayout input;
        input.setBindings({QRhiVertexInputBinding(6 * sizeof(float))});
        input.setAttributes(
            {QRhiVertexInputAttribute(0, 0, QRhiVertexInputAttribute::Float2, 0),
             QRhiVertexInputAttribute(0, 1, QRhiVertexInputAttribute::Float4, 2 * sizeof(float))});
        pipeline->setVertexInputLayout(input);
        pipeline->setShaderResourceBindings(retouchBindings_.get());
        pipeline->setRenderPassDescriptor(pass_.get());
        if (!pipeline->create())
            return;
        retouchPipeline_ = std::move(pipeline);
    }
    const auto bytes = int(vertices.size() * qsizetype(sizeof(float)));
    if (bytes > retouchCapacity_) {
        retouchVertices_.reset(
            rhi_->newBuffer(QRhiBuffer::Dynamic, QRhiBuffer::VertexBuffer, bytes));
        if (!retouchVertices_->create()) {
            retouchCapacity_ = 0;
            return;
        }
        retouchCapacity_ = bytes;
    }
    const std::array<float, 4> viewport{float(width()), float(height()),
                                        rhi_->isYUpInNDC() ? -1.F : 1.F, 0};
    updates->updateDynamicBuffer(retouchUniform_.get(), 0, 16, viewport.data());
    updates->updateDynamicBuffer(retouchVertices_.get(), 0, bytes, vertices.constData());
    retouchVertexCount_ = int(vertices.size() / 6);
}
void Canvas::drawRetouch(QRhiCommandBuffer* cb) {
    if (retouchVertexCount_ <= 0 || !retouchPipeline_)
        return;
    cb->setGraphicsPipeline(retouchPipeline_.get());
    const auto size = swapchain_->currentPixelSize();
    cb->setViewport({0, 0, float(size.width()), float(size.height())});
    cb->setShaderResources(retouchBindings_.get());
    const QRhiCommandBuffer::VertexInput binding(retouchVertices_.get(), 0);
    cb->setVertexInput(0, 1, &binding);
    cb->draw(retouchVertexCount_);
}

void Canvas::retryRender() {
    if (!isExposed() || renderRetry_.isActive()) return;
    renderRetry_.start(renderRetryMs_);
    renderRetryMs_ = std::min(renderRetryMs_ * 2, 4000);
}
void Canvas::render() {
    if (!isExposed() || width() <= 0 || height() <= 0) return;
    if (!initialize()) { retryRender(); return; }
    if (!configured_)
        configureOutput();
    if (!configured_) { retryRender(); return; }
    if (swapchain_->currentPixelSize() != swapchain_->surfacePixelSize() &&
        !swapchain_->createOrResize()) {
        emit displayStatus(tr("Cannot create display swapchain"));
        retryRender();
        return;
    }
    auto result = rhi_->beginFrame(swapchain_.get());
    if (result == QRhi::FrameOpSwapChainOutOfDate) {
        if (!swapchain_->createOrResize()) {
            emit displayStatus(tr("Cannot create display swapchain"));
            retryRender();
            return;
        }
        result = rhi_->beginFrame(swapchain_.get());
    }
    if (result != QRhi::FrameOpSuccess) {
        release();
        emit displayStatus(tr("Display device lost; renderer will retry"));
        retryRender();
        return;
    }
    auto* updates = rhi_->nextResourceUpdateBatch();
    bool rebind = !bindings_;
    for (size_t i = 0; i < textures_.size(); ++i) {
        const auto p = aigle_frame_pixels(frames_[i].get());
        const QSize size = frames_[i] ? QSize(int(p.width), int(p.height)) : QSize(1, 1);
        const auto format = p.format == 1 ? QRhiTexture::RGBA16F : QRhiTexture::BGRA8;
        if (!textures_[i] || textures_[i]->pixelSize() != size ||
            textures_[i]->format() != format) {
            bindings_.reset();
            pipeline_.reset();
            textures_[i].reset(rhi_->newTexture(format, size));
            if (!textures_[i]->create()) {
                updates->release();
                rhi_->endFrame(swapchain_.get());
                emit displayStatus(tr("Cannot allocate image texture"));
                return;
            }
            rebind = true;
            upload_[i] = true;
        }
        if (upload_[i]) {
            const std::array<char, 4> empty{};
            const auto bytes =
                frames_[i] ? QByteArray(reinterpret_cast<const char*>(p.data), qsizetype(p.length))
                           : QByteArray(empty.data(), 4);
            QRhiTextureSubresourceUploadDescription description(bytes);
            if (frames_[i])
                description.setDataStride(p.stride);
            updates->uploadTexture(textures_[i].get(),
                                   QRhiTextureUploadDescription{{0, 0, description}});
            upload_[i] = false;
        }
    }
    if (rebind) {
        bindings_.reset(rhi_->newShaderResourceBindings());
        bindings_->setBindings(
            {QRhiShaderResourceBinding::uniformBuffer(0, QRhiShaderResourceBinding::FragmentStage,
                                                      uniform_.get()),
             QRhiShaderResourceBinding::sampledTexture(1, QRhiShaderResourceBinding::FragmentStage,
                                                       textures_[0].get(), sampler_.get()),
             QRhiShaderResourceBinding::sampledTexture(2, QRhiShaderResourceBinding::FragmentStage,
                                                       textures_[1].get(), sampler_.get())});
        if (!bindings_->create()) {
            updates->release();
            rhi_->endFrame(swapchain_.get());
            return;
        }
        pipeline_.reset(rhi_->newGraphicsPipeline());
        pipeline_->setShaderStages({{QRhiShaderStage::Vertex, shader("canvas.vert")},
                                    {QRhiShaderStage::Fragment, shader("canvas.frag")}});
        pipeline_->setVertexInputLayout({});
        pipeline_->setShaderResourceBindings(bindings_.get());
        pipeline_->setRenderPassDescriptor(pass_.get());
        if (!pipeline_->create()) {
            pipeline_.reset();
            updates->release();
            rhi_->endFrame(swapchain_.get());
            return;
        }
    }
    QPointF a = dragStart_, b = lastMouse_;
    int shape = dragging_ && tool_ != "pan" ? (tool_ == "linear" ? 3 : 2) : 0;
    if (tool_ == "retouch")
        shape = 0;
    if (tool_ == "crop") {
        shape = cropGuide_ == 0 ? 1 : (cropGuide_ == 1 ? 5 : 6);
        a = windowPoint(QPointF(cropDraft_.left() * viewport_.image.width(),
                                cropDraft_.top() * viewport_.image.height()));
        b = windowPoint(QPointF(cropDraft_.right() * viewport_.image.width(),
                                cropDraft_.bottom() * viewport_.image.height()));
    } else if (tool_ == "mask_edit" && maskGuide_.valid) {
        a = windowPoint(maskGuide_.handles[0].value_or(QPointF{}));
        b = windowPoint(maskGuide_.handles[1].value_or(QPointF{}));
        shape = maskGuide_.kind == "linear"   ? 3
                : maskGuide_.kind == "radial" ? 7
                                                       : 8;
    }
    if (tool_ == "crop" && horizon_ && dragging_) {
        shape = 3;
        a = dragStart_;
        b = lastMouse_;
    }
    a = {a.x() / width(), a.y() / height()};
    b = {b.x() / width(), b.y() / height()};
    std::array<QVector4D, 70> uniforms{
        mapping(frames_[0]), mapping(frames_[1]),
        QVector4D(hdrActive() ? 1.F : 0.F, float(encoding_), original_ ? 2.F : (compare_ ? 0.5F : -1.F),
                  rhi_->isYUpInNDC() ? 0.F : 1.F),
        QVector4D(float(a.x()), float(a.y()), float(b.x()), float(b.y())),
        QVector4D(float(shape), float(width()), float(height()),
                  float((shape == 1 || shape == 5 || shape == 6)
                            ? (cropAngle_ - editParams_.angle) *
                                  3.141592653589793 / 180
                            : radius_ * viewport_.image.width() * viewport_.scale() /
                                  devicePixelRatio()))};
    if (shape == 1 || shape == 5 || shape == 6) {
        const auto center = windowPoint({viewport_.image.width() / 2, viewport_.image.height() / 2});
        uniforms[5] = QVector4D(float(center.x() / width()), float(center.y() / height()), 0, 0);
    }
    if (shape == 7) {
        const auto pt = windowPoint(maskGuide_.handles[2].value_or(QPointF{}));
        uniforms[5] = QVector4D(float(pt.x()), float(pt.y()), 0, 0);
        const auto& outline = maskGuide_.outline;
        for (int i = 0; i < 64 && i + 1 < outline.size(); ++i) {
            const auto p = windowPoint(outline[i]);
            const auto q = windowPoint(outline[i + 1]);
            uniforms[6 + i] = QVector4D(float(p.x()), float(p.y()), float(q.x()), float(q.y()));
        }
    }
    updates->updateDynamicBuffer(uniform_.get(), 0, 1120, uniforms.data());
    prepareRetouch(updates);
    auto* cb = swapchain_->currentFrameCommandBuffer();
    cb->beginPass(swapchain_->currentFrameRenderTarget(), QColor(24, 24, 24), {1, 0}, updates);
    if (pipeline_) {
        const auto size = swapchain_->currentPixelSize();
        cb->setGraphicsPipeline(pipeline_.get());
        cb->setViewport({0, 0, float(size.width()), float(size.height())});
        cb->setShaderResources(bindings_.get());
        cb->draw(3);
    }
    drawRetouch(cb);
    cb->endPass();
    const bool capture = !capturePath_.isEmpty() && !hdrActive();
    if (capture) {
        const auto path = std::exchange(capturePath_, {});
        readback_.completed = [this, path] {
            QImage image(reinterpret_cast<const uchar*>(readback_.data.constData()),
                         readback_.pixelSize.width(), readback_.pixelSize.height(),
                         readback_.format == QRhiTexture::BGRA8 ? QImage::Format_ARGB32
                                                                : QImage::Format_RGBA8888);
            image = image.copy();
            if (rhi_->isYUpInFramebuffer())
                image.flip(Qt::Vertical);
            image.save(path);
            emit captured(image);
        };
        auto* read = rhi_->nextResourceUpdateBatch();
        read->readBackTexture({}, &readback_);
        cb->resourceUpdate(read);
    }
    const auto ended = rhi_->endFrame(swapchain_.get());
    if (ended != QRhi::FrameOpSuccess) {
        release();
        emit displayStatus(tr("Display device lost; renderer will retry"));
        retryRender();
        return;
    }
    renderRetry_.stop();
    renderRetryMs_ = 250;
    if (capture)
        rhi_->finish();
    if (frames_[0])
        emit rendered(hdrActive(), aigle_frame_pixels(frames_[0].get()).revision);
}
void Canvas::wheelEvent(QWheelEvent* event) {
    cancelDrag();
    viewport_.zoom(viewport_.scale() * std::pow(1.2, event->angleDelta().y() / 120.0),
                   event->position() * devicePixelRatio());
    sendViewport(true);
    settle_.start();
    requestUpdate();
    event->accept();
}
void Canvas::mousePressEvent(QMouseEvent* event) {
    if (event->button() != Qt::LeftButton)
        return;
    requestActivate();
    dragging_ = true;
    dragStart_ = lastMouse_ = event->position();
    toolPoints_ = {};
    const auto point = imagePoint(event->position());
    if (tool_ == "retouch") {
        retouchHandle_.clear();
        retouchPreview_ = {};
        const auto distance = [this, event](QPointF value) {
            return QLineF(windowPoint(value), event->position()).length();
        };
        const auto polygon = [this](const QList<QPointF>& points) {
            QPolygonF result;
            for (const auto vertex : points) result.append(windowPoint(vertex));
            return result;
        };
        const auto edgeDistance = [&](const QPolygonF& outline) {
            double nearestDistance = std::numeric_limits<double>::infinity();
            for (qsizetype i = 1; i < outline.size(); ++i) {
                const auto a = outline[i - 1], d = outline[i] - a;
                const auto length = QPointF::dotProduct(d, d);
                const auto f =
                    length > 0 ? std::clamp(QPointF::dotProduct(event->position() - a, d) / length,
                                            0.0, 1.0)
                               : 0.0;
                nearestDistance = std::min(nearestDistance, QLineF(event->position(), a + d * f).length());
            }
            return nearestDistance;
        };
        const bool forceNew = event->modifiers().testFlag(Qt::AltModifier);
        const auto current = retouchGuide();
        if (!forceNew && current.valid) {
            double nearest = 12;
            for (const auto& handle : QStringList{"target", "source", "radius_handle"}) {
                if (handle == "source" && !current.source) continue;
                const auto d = distance(handle == "target" ? current.target : handle == "source" ? *current.source : current.radiusHandle);
                // The radius grip uses the same tolerance as the rim. A wider
                // grip steals clicks from the correction body (move target).
                if (d < nearest && (handle != "radius_handle" || d < 6)) {
                    nearest = d;
                    retouchHandle_ = handle == "radius_handle" ? "radius" : handle;
                }
            }
            const auto target = polygon(current.outline),
                       source = polygon(current.sourceOutline);
            const auto rim = edgeDistance(target);
            if (rim < 6 && rim < nearest) {
                retouchHandle_ = "radius";
                nearest = rim;
            }
            if (current.source.has_value() && edgeDistance(source) < std::min(6.0, nearest))
                retouchHandle_ = "source";
            if (retouchHandle_.isEmpty()) {
                if (target.containsPoint(event->position(), Qt::OddEvenFill))
                    retouchHandle_ = "target";
                else if (source.containsPoint(event->position(), Qt::OddEvenFill))
                    retouchHandle_ = "source";
            }
        }
        if (!forceNew && retouchHandle_.isEmpty()) {
            for (qsizetype i = retouchGuides_.size(); i-- > 0;) {
                const auto guide = retouchGuides_[i];
                const auto outline = polygon(guide.outline);
                if (distance(guide.target) < 12 ||
                    outline.containsPoint(event->position(), Qt::OddEvenFill) ||
                    edgeDistance(outline) < 7) {
                    retouchKind_ = guide.kind;
                    retouchIndex_ = guide.index;
                    retouchHandle_ = "target";
                    emit retouchSelected(retouchKind_, retouchIndex_);
                    break;
                }
            }
        }
        retouchSession_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
        if (!retouchHandle_.isEmpty())
            retouchStart_ = retouchGuide();
        else {
            retouchKind_.clear();
            retouchIndex_ = -1;
            emit retouchSelected({}, -1);
            retouchPending_ = retouchRequest(commands::Retouch::Phase::Placement);
            retouchPending_->tool = retouchDefaults_.tool == RetouchDefaults::Tool::RedEye ? "red_eye" : retouchDefaults_.tool == RetouchDefaults::Tool::Clone ? "clone" : "heal";
            retouchPending_->radius = retouchDefaults_.radius;
            retouchPending_->feather = retouchDefaults_.feather;
            retouchPending_->opacity = retouchDefaults_.opacity;
            retouchPending_->pupil = retouchDefaults_.pupil;
            retouchPending_->darken = retouchDefaults_.darken;
            retouchTimer_.start();
        }
    } else if (tool_ == "pan")
        setCursor(Qt::ClosedHandCursor);
    else if (tool_ == "crop") {
        cropStart_ = cropDraft_;
        cropAngleStart_ = cropAngle_;
        const auto lo = windowPoint({cropDraft_.left() * viewport_.image.width(),
                                     cropDraft_.top() * viewport_.image.height()});
        const auto hi = windowPoint({cropDraft_.right() * viewport_.image.width(),
                                     cropDraft_.bottom() * viewport_.image.height()});
        dragPart_ = 0;
        if (QLineF(event->position(), lo).length() < 16)
            dragPart_ = 1;
        else if (QLineF(event->position(), {hi.x(), lo.y()}).length() < 16)
            dragPart_ = 2;
        else if (QLineF(event->position(), hi).length() < 16)
            dragPart_ = 3;
        else if (QLineF(event->position(), {lo.x(), hi.y()}).length() < 16)
            dragPart_ = 4;
        else if (QLineF(event->position(), {(lo.x() + hi.x()) / 2, lo.y()}).length() < 14)
            dragPart_ = 5;
        else if (QLineF(event->position(), {hi.x(), (lo.y() + hi.y()) / 2}).length() < 14)
            dragPart_ = 6;
        else if (QLineF(event->position(), {(lo.x() + hi.x()) / 2, hi.y()}).length() < 14)
            dragPart_ = 7;
        else if (QLineF(event->position(), {lo.x(), (lo.y() + hi.y()) / 2}).length() < 14)
            dragPart_ = 8;
        else if (!QRectF(lo, hi).contains(event->position())) {
            dragPart_ = 9;
            cropPivot_ = windowPoint({cropDraft_.center().x() * viewport_.image.width(),
                                      cropDraft_.center().y() * viewport_.image.height()});
            cropPointerAngle_ = std::atan2(event->position().y() - cropPivot_.y(),
                                           event->position().x() - cropPivot_.x());
            cropAngleStart_ = cropAngle_;
        }
    } else if (tool_ == "mask_edit") {
        maskStart_ = maskGuide_;
        dragPart_ = -1;
        for (size_t i = 0; i < maskGuide_.handles.size(); ++i) {
            if (maskGuide_.handles[i] && QLineF(event->position(), windowPoint(*maskGuide_.handles[i])).length() < 14)
                dragPart_ = int(i) + 1;
        }
        if (maskGuide_.kind == "linear") {
            const auto center = windowPoint((maskGuide_.handles[0].value_or(QPointF{}) + maskGuide_.handles[1].value_or(QPointF{})) / 2);
            if (QLineF(event->position(), center).length() < 10)
                dragPart_ = 0;
        }
        if (maskGuide_.kind == "move" && dragPart_ > 0)
            dragPart_ = 1;
        if (dragPart_ < 0) {
            dragging_ = false;
            return;
        }
    }
    toolPoints_.append(point);
    requestUpdate();
}
void Canvas::mouseMoveEvent(QMouseEvent* event) {
    if (!dragging_)
        return;
    if (tool_ == "pan") {
        viewport_.pan((event->position() - lastMouse_) * devicePixelRatio());
        sendViewport(true);
    } else if (tool_ == "crop" && !horizon_) {
        auto point = imagePoint(event->position());
        point = {point.x() / viewport_.image.width(),
                 point.y() / viewport_.image.height()};
        auto start = imagePoint(dragStart_);
        start = {start.x() / viewport_.image.width(), start.y() / viewport_.image.height()};
        if (dragPart_ == 0) {
            cropDraft_ = constrainedCrop(cropStart_, dragPart_, point - start);
        } else if (dragPart_ == 9) {
            double angle = std::atan2(event->position().y() - cropPivot_.y(),
                                      event->position().x() - cropPivot_.x()) - cropPointerAngle_;
            if (angle > std::numbers::pi) angle -= 2 * std::numbers::pi;
            if (angle < -std::numbers::pi) angle += 2 * std::numbers::pi;
            cropDraft_ = cropStart_;
            setCropAngle(std::clamp(cropAngleStart_ + angle * 180 / std::numbers::pi, -45., 45.));
            emit cropAngleUpdated(cropAngle_);
        } else {
            cropDraft_ = constrainedCrop(cropStart_, dragPart_, point - start);
        }
    } else if (tool_ == "retouch") {
        if (!retouchHandle_.isEmpty()) {
            lastMouse_ = event->position();
            retouchPending_ = retouchRequest(commands::Retouch::Phase::Preview);
            if (!retouchTimer_.isActive())
                retouchTimer_.start();
        } else {
            lastMouse_ = event->position();
            retouchPending_ = retouchRequest(commands::Retouch::Phase::Placement);
            retouchPending_->tool = retouchDefaults_.tool == RetouchDefaults::Tool::RedEye ? "red_eye" : retouchDefaults_.tool == RetouchDefaults::Tool::Clone ? "clone" : "heal";
            retouchPending_->radius = retouchDefaults_.radius;
            retouchPending_->feather = retouchDefaults_.feather;
            retouchPending_->opacity = retouchDefaults_.opacity;
            retouchPending_->pupil = retouchDefaults_.pupil;
            retouchPending_->darken = retouchDefaults_.darken;
            if (!retouchTimer_.isActive())
                retouchTimer_.start();
        }
    } else if (tool_ == "mask_edit" && maskGuide_.valid) {
        const auto delta = imagePoint(event->position()) - imagePoint(dragStart_);
        const bool linear = maskGuide_.kind == "linear";
        const bool moving = (!linear && dragPart_ == 1) || (linear && dragPart_ == 0);
        for (size_t i = 0; i < maskStart_.handles.size(); ++i) {
            if (!maskStart_.handles[i] || (!moving && dragPart_ != int(i) + 1)) continue;
            maskGuide_.handles[i] = *maskStart_.handles[i] + delta;
        }
        if (moving) {
            maskGuide_.outline = maskStart_.outline;
            for (auto& point : maskGuide_.outline) point += delta;
        }

    } else {
        const auto p = imagePoint(event->position());
        toolPoints_.append(p);
    }
    lastMouse_ = event->position();
    requestUpdate();
}
void Canvas::mouseReleaseEvent(QMouseEvent* event) {
    if (!dragging_)
        return;
    mouseMoveEvent(event);
    dragging_ = false;
    if (tool_ == "crop" && horizon_) {
        const auto delta = event->position() - dragStart_;
        if (QLineF(dragStart_, event->position()).length() >= 8) {
            double angle = std::atan2(delta.y(), delta.x()) * 180 / 3.141592653589793;
            while (angle > 45)
                angle -= 90;
            while (angle < -45)
                angle += 90;
            setCropAngle(
                std::clamp(editParams_.angle - angle, -45., 45.));
            emit cropAngleUpdated(cropAngle_);
        }
        horizon_ = false;
    } else if (tool_ == "pan") {
        setCursor(Qt::OpenHandCursor);
        sendViewport();
    } else if (tool_ == "mask_edit" && maskGuide_.valid) {
        if (QLineF(dragStart_, event->position()).length() < 1) {
            requestUpdate();
            return;
        }
        QString handle;
        QList<QPointF> points{maskGuide_.handles[0].value_or(QPointF{}), maskGuide_.handles[1].value_or(QPointF{})};
        if (maskGuide_.kind != "linear" || dragPart_ == 0) {
            handle = dragPart_ <= 1 ? "move" : dragPart_ == 2 ? "radius_x" : "radius_y";
            const auto start = imagePoint(dragStart_), end = imagePoint(event->position());
            points = handle == "move" ? QList<QPointF>{start, end}
                                      : QList<QPointF>{maskGuide_.handles[0].value_or(QPointF{}),
                                                   maskGuide_.handles[dragPart_ == 2 ? 1 : 2].value_or(QPointF{})};
        }
        emit toolCommand(commands::Tool{.tool = "mask_transform", .points = points, .id = maskGuide_.id, .handle = handle});
    } else if (tool_ == "retouch") {
        retouchTimer_.stop();
        retouchPending_ = {};
        if (!retouchHandle_.isEmpty()) {
            if (QLineF(dragStart_, lastMouse_).length() >= 1)
                emit toolCommand(retouchRequest(commands::Retouch::Phase::Commit));
            else
                emit toolCommand(commands::RetouchCancel{.session = retouchSession_});
            retouchHandle_.clear();
        } else {
            const auto p = imagePoint(event->position());
            toolPoints_.append(p);
            commands::Tool request;
            request.tool = retouchDefaults_.tool == RetouchDefaults::Tool::RedEye ? "red_eye" : retouchDefaults_.tool == RetouchDefaults::Tool::Clone ? "clone" : "heal";
            request.radius = retouchDefaults_.radius;
            request.feather = retouchDefaults_.feather;
            request.opacity = retouchDefaults_.opacity;
            request.pupil = retouchDefaults_.pupil;
            request.darken = retouchDefaults_.darken;
            request.points = toolPoints_;
            request.document_id = qint64(retouchDocument_);
            request.revision = qint64(retouchRevision_);
            emit toolCommand(request);
        }
    } else if (tool_ != "crop" && tool_ != "mask_edit") {
        const auto p = imagePoint(event->position());
        toolPoints_.append(p);
        emit toolCommand(
            commands::Tool{.tool = tool_, .points = toolPoints_, .radius = radius_});
    }
    requestUpdate();
}
void Canvas::mouseDoubleClickEvent(QMouseEvent* event) {
    if (event->button() != Qt::LeftButton) return;
    if (tool_ == "crop") {
        const auto lo = windowPoint({cropDraft_.left() * viewport_.image.width(),
                                     cropDraft_.top() * viewport_.image.height()});
        const auto hi = windowPoint({cropDraft_.right() * viewport_.image.width(),
                                     cropDraft_.bottom() * viewport_.image.height()});
        const QRectF bounds(lo, hi);
        if (!bounds.contains(event->position())) return;
        for (const auto& fraction : {QPointF(0, 0), QPointF(1, 0), QPointF(1, 1), QPointF(0, 1),
                                     QPointF(.5, 0), QPointF(1, .5), QPointF(.5, 1), QPointF(0, .5)}) {
            const auto grip = lo + QPointF(bounds.width() * fraction.x(), bounds.height() * fraction.y());
            if (QLineF(grip, event->position()).length() < 16) return;
        }
        dragging_ = false;
        horizon_ = false;
        applyCrop();
        return;
    }
    oneToOne();
}
} // namespace aigle
